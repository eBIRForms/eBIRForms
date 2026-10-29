//! Exact editable-save contract for form `1701Qv2018`.
//!
//! The field order and encoding behavior come from resource 170,
//! `forms\BIR-Form1701Qv2018.hta`, in the hash-locked eBIRForms 7.9.5.0
//! package. This module deliberately implements `saveXML(false)`, not the
//! credential-bearing `saveEncryptedProfile`/FTP submission path.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use super::FormValidator;
use super::form_1701q::{
    Form1701QAmounts, Form1701QAtc, Form1701QDeductionMethod, Form1701QDraft, Form1701QFilerType,
    Form1701QParty, Form1701QPaymentDetails, Form1701QPaymentRow, Form1701QSpouseType,
    Form1701QTaxRate, USER_ENTERED_AMOUNT_ITEMS,
};

const EDITABLE_XML_CLOSE: &str = "All Rights Reserved BIR 2012.";
const XML_FORMAT: &str = "\t\r\n            ";
/// Deny-list marker, not a taxpayer to load. Never import or emit this TIN.
const BLOCKED_SAVE_TIN_DIGITS: &str = "000123453";
const BLOCKED_SAVE_TIN_DASHED: &str = "000-123-453";
const JAVASCRIPT_ESCAPED_FIELDS: [&str; 3] = [
    "frm1701q:txtTaxpayerName",
    "frm1701q:txtAddress",
    "frm1701q:txtLOB",
];
const PAIRED_AMOUNT_ITEMS: &[u8] = &[
    26, 27, 28, 29, 30, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54,
    55, 56, 57, 58, 59, 60, 61, 62, 63, 64, 65, 66, 67, 68,
];
/// The amounts checked against the recompute after an import. Item 52 is
/// typed and restored (USER_ENTERED_AMOUNT_ITEMS), but the compute zeroes it
/// unless its column is on the 8% rate with an ATC other than II016
/// (`item52Validate`): a save file carrying a non-zero 52 there is refused,
/// as it was while 52 was only checked.
const DERIVED_AMOUNT_ITEMS: &[u8] = &[
    26, 27, 28, 29, 30, 38, 40, 41, 45, 46, 49, 51, 52, 53, 54, 62, 63, 67, 68,
];

/// The exact 172 fields emitted by `saveXML(false)`, in live `frmMain.elements`
/// order. The two dynamic RDO selects are included where their host cells occur;
/// `txtAddress2` is omitted because the HTA appends it to `txtAddress`.
pub const OFFICIAL_EDITABLE_FIELD_IDS: &[&str] = &[
    "frm1701q:txtYear",
    "frm1701q:DateQuarter_1",
    "frm1701q:DateQuarter_2",
    "frm1701q:DateQuarter_3",
    "frm1701q:AmendedRtn_1",
    "frm1701q:AmendedRtn_2",
    "frm1701q:txtSheets",
    "frm1701q:txtTIN1",
    "frm1701q:txtTIN2",
    "frm1701q:txtTIN3",
    "frm1701q:txtBranchCode",
    "frm1701q:txtRDOCode",
    "frm1701q:optType_1",
    "frm1701q:optType_2",
    "frm1701q:optType_3",
    "frm1701q:optType_4",
    "frm1701q:optATC_1",
    "frm1701q:optATC_2",
    "frm1701q:optATC_3",
    "frm1701q:optATC_4",
    "frm1701q:optATC_5",
    "frm1701q:optATC_6",
    "frm1701q:txtTaxpayerName",
    "frm1701q:txtAddress",
    "frm1701q:txtZipCode",
    "frm1701q:txtBirthMonth",
    "frm1701q:txtBirthDay",
    "frm1701q:txtBirthYear",
    "txtEmail",
    "frm1701q:txtCitizenship",
    "frm1701q:txtForeignTaxNumber",
    "frm1701q:optForeignTaxCredits_1",
    "frm1701q:optForeignTaxCredits_2",
    "frm1701q:optTaxRate_1",
    "frm1701q:optMethodOfDeduction:_1",
    "frm1701q:optMethodOfDeduction:_2",
    "frm1701q:optTaxRate_2",
    "frm1701q:txtSpouseTIN1",
    "frm1701q:txtSpouseTIN2",
    "frm1701q:txtSpouseTIN3",
    "frm1701q:txtSpouseBranchCode",
    "frm1701q:txtSpouseRDOCode",
    "frm1701q:optSpouseType_1",
    "frm1701q:optSpouseType_2",
    "frm1701q:optSpouseType_3",
    "frm1701q:optSpouseATC_1",
    "frm1701q:optSpouseATC_2",
    "frm1701q:optSpouseATC_3",
    "frm1701q:optSpouseATC_4",
    "frm1701q:optSpouseATC_5",
    "frm1701q:optSpouseATC_6",
    "frm1701q:optSpouseATC_7",
    "frm1701q:txtSpouseName",
    "frm1701q:txtSpouseCitizenship",
    "frm1701q:txtSpouseForeignTaxNum",
    "frm1701q:optSpouseForeignTaxCred_1",
    "frm1701q:optSpouseForeignTaxCred_2",
    "frm1701q:optSpouseTaxRate_1",
    "frm1701q:optSpouseMethod:_1",
    "frm1701q:optSpouseMethod:_2",
    "frm1701q:optSpouseTaxRate_2",
    "frm1701q:txt26A",
    "frm1701q:txt26B",
    "frm1701q:txt27A",
    "frm1701q:txt27B",
    "frm1701q:txt28A",
    "frm1701q:txt28B",
    "frm1701q:txt29A",
    "frm1701q:txt29B",
    "frm1701q:txt30A",
    "frm1701q:txt30B",
    "frm1701q:txt31",
    "frm1701q:txtAgency32",
    "frm1701q:txtNumber32",
    "frm1701q:txtDate32",
    "frm1701q:txtAmount32",
    "frm1701q:txtAgency33",
    "frm1701q:txtNumber33",
    "frm1701q:txtDate33",
    "frm1701q:txtAmount33",
    "frm1701q:txtNumber34",
    "frm1701q:txtDate34",
    "frm1701q:txtAmount34",
    "frm1701q:txtParticular35",
    "frm1701q:txtAgency35",
    "frm1701q:txtNumber35",
    "frm1701q:txtDate35",
    "frm1701q:txtAmount35",
    "frm1701q:txtPg2TIN1",
    "frm1701q:txtPg2TIN2",
    "frm1701q:txtPg2TIN3",
    "frm1701q:txtPg2BranchCode",
    "frm1701q:txtPg2TaxpayerName",
    "frm1701q:txt36A",
    "frm1701q:txt36B",
    "frm1701q:txt37A",
    "frm1701q:txt37B",
    "frm1701q:txt38A",
    "frm1701q:txt38B",
    "frm1701q:txt39A",
    "frm1701q:txt39B",
    "frm1701q:txt40A",
    "frm1701q:txt40B",
    "frm1701q:txt41A",
    "frm1701q:txt41B",
    "frm1701q:txt42A",
    "frm1701q:txt42B",
    "frm1701q:txt43Desc",
    "frm1701q:txt43A",
    "frm1701q:txt43B",
    "frm1701q:txt44A",
    "frm1701q:txt44B",
    "frm1701q:txt45A",
    "frm1701q:txt45B",
    "frm1701q:txt46A",
    "frm1701q:txt46B",
    "frm1701q:txt47A",
    "frm1701q:txt47B",
    "frm1701q:txt48Desc",
    "frm1701q:txt48A",
    "frm1701q:txt48B",
    "frm1701q:txt49A",
    "frm1701q:txt49B",
    "frm1701q:txt50A",
    "frm1701q:txt50B",
    "frm1701q:txt51A",
    "frm1701q:txt51B",
    "frm1701q:txt52A",
    "frm1701q:txt52B",
    "frm1701q:txt53A",
    "frm1701q:txt53B",
    "frm1701q:txt54A",
    "frm1701q:txt54B",
    "frm1701q:txt55A",
    "frm1701q:txt55B",
    "frm1701q:txt56A",
    "frm1701q:txt56B",
    "frm1701q:txt57A",
    "frm1701q:txt57B",
    "frm1701q:txt58A",
    "frm1701q:txt58B",
    "frm1701q:txt59A",
    "frm1701q:txt59B",
    "frm1701q:txt60A",
    "frm1701q:txt60B",
    "frm1701q:txt61Desc",
    "frm1701q:txt61A",
    "frm1701q:txt61B",
    "frm1701q:txt62A",
    "frm1701q:txt62B",
    "frm1701q:txt63A",
    "frm1701q:txt63B",
    "frm1701q:txt64A",
    "frm1701q:txt64B",
    "frm1701q:txt65A",
    "frm1701q:txt65B",
    "frm1701q:txt66A",
    "frm1701q:txt66B",
    "frm1701q:txt67A",
    "frm1701q:txt67B",
    "frm1701q:txt68A",
    "frm1701q:txt68B",
    "frm1701q:txtCurrentPage",
    "frm1701q:txtMaxPage",
    "txtFinalFlag",
    "txtEnroll",
    "ebirOnlineConfirmUsername",
    "ebirOnlineUsername",
    "ebirOnlineSecret",
    "frm1701q:txtLOB",
    "frm1701q:txtTelno",
    "driveSelectTPExport",
];

impl Form1701QDraft {
    pub fn to_bir_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = BTreeMap::new();
        let (tin1, tin2, tin3, branch) = split_tin(&self.tin);
        let (spouse_tin1, spouse_tin2, spouse_tin3, spouse_branch) = split_tin(&self.spouse_tin);
        let (birth_month, birth_day, birth_year) = split_date(&self.date_of_birth);

        insert(
            &mut fields,
            "frm1701q:txtYear",
            self.taxable_year.to_string(),
        );
        for quarter in 1..=3 {
            insert_bool(
                &mut fields,
                &format!("frm1701q:DateQuarter_{quarter}"),
                self.quarter == quarter,
            );
        }
        insert_bool(&mut fields, "frm1701q:AmendedRtn_1", self.is_amended);
        insert_bool(&mut fields, "frm1701q:AmendedRtn_2", !self.is_amended);
        insert(
            &mut fields,
            "frm1701q:txtSheets",
            self.number_of_sheets.to_string(),
        );
        insert(&mut fields, "frm1701q:txtTIN1", tin1.clone());
        insert(&mut fields, "frm1701q:txtTIN2", tin2.clone());
        insert(&mut fields, "frm1701q:txtTIN3", tin3.clone());
        insert(&mut fields, "frm1701q:txtBranchCode", branch.clone());
        insert(&mut fields, "frm1701q:txtRDOCode", self.rdo_code.clone());

        insert_choice(
            &mut fields,
            &[
                ("frm1701q:optType_1", Form1701QFilerType::SingleProprietor),
                ("frm1701q:optType_2", Form1701QFilerType::Professional),
                ("frm1701q:optType_3", Form1701QFilerType::Estate),
                ("frm1701q:optType_4", Form1701QFilerType::Trust),
            ],
            self.filer_type,
        );
        insert_choice(
            &mut fields,
            &[
                ("frm1701q:optATC_1", Form1701QAtc::Ii012),
                ("frm1701q:optATC_2", Form1701QAtc::Ii014),
                ("frm1701q:optATC_3", Form1701QAtc::Ii013),
                ("frm1701q:optATC_4", Form1701QAtc::Ii015),
                ("frm1701q:optATC_5", Form1701QAtc::Ii017),
                ("frm1701q:optATC_6", Form1701QAtc::Ii016),
            ],
            self.atc,
        );
        insert(
            &mut fields,
            "frm1701q:txtTaxpayerName",
            self.taxpayer_name.clone(),
        );
        insert(
            &mut fields,
            "frm1701q:txtAddress",
            format!("{}{}", self.registered_address, self.registered_address_2),
        );
        insert(&mut fields, "frm1701q:txtZipCode", self.zip_code.clone());
        insert(&mut fields, "frm1701q:txtBirthMonth", birth_month);
        insert(&mut fields, "frm1701q:txtBirthDay", birth_day);
        insert(&mut fields, "frm1701q:txtBirthYear", birth_year);
        insert(&mut fields, "txtEmail", self.email.clone());
        insert(
            &mut fields,
            "frm1701q:txtCitizenship",
            self.citizenship.clone(),
        );
        insert(
            &mut fields,
            "frm1701q:txtForeignTaxNumber",
            self.foreign_tax_number.clone(),
        );
        insert_optional_bool_pair(
            &mut fields,
            "frm1701q:optForeignTaxCredits_1",
            "frm1701q:optForeignTaxCredits_2",
            self.claims_foreign_tax_credits,
        );
        insert_bool(
            &mut fields,
            "frm1701q:optTaxRate_1",
            self.tax_rate == Some(Form1701QTaxRate::Graduated),
        );
        insert_bool(
            &mut fields,
            "frm1701q:optMethodOfDeduction:_1",
            self.deduction_method == Some(Form1701QDeductionMethod::Itemized),
        );
        insert_bool(
            &mut fields,
            "frm1701q:optMethodOfDeduction:_2",
            self.deduction_method == Some(Form1701QDeductionMethod::Osd),
        );
        insert_bool(
            &mut fields,
            "frm1701q:optTaxRate_2",
            self.tax_rate == Some(Form1701QTaxRate::EightPercent),
        );

        insert(&mut fields, "frm1701q:txtSpouseTIN1", spouse_tin1);
        insert(&mut fields, "frm1701q:txtSpouseTIN2", spouse_tin2);
        insert(&mut fields, "frm1701q:txtSpouseTIN3", spouse_tin3);
        insert(&mut fields, "frm1701q:txtSpouseBranchCode", spouse_branch);
        insert(
            &mut fields,
            "frm1701q:txtSpouseRDOCode",
            self.spouse_rdo_code.clone(),
        );
        insert_choice(
            &mut fields,
            &[
                (
                    "frm1701q:optSpouseType_1",
                    Form1701QSpouseType::SingleProprietor,
                ),
                (
                    "frm1701q:optSpouseType_2",
                    Form1701QSpouseType::Professional,
                ),
                (
                    "frm1701q:optSpouseType_3",
                    Form1701QSpouseType::CompensationEarner,
                ),
            ],
            self.has_spouse.then_some(self.spouse_type).flatten(),
        );
        insert_choice(
            &mut fields,
            &[
                ("frm1701q:optSpouseATC_1", Form1701QAtc::Ii012),
                ("frm1701q:optSpouseATC_2", Form1701QAtc::Ii014),
                ("frm1701q:optSpouseATC_3", Form1701QAtc::Ii013),
                ("frm1701q:optSpouseATC_4", Form1701QAtc::Ii011),
                ("frm1701q:optSpouseATC_5", Form1701QAtc::Ii015),
                ("frm1701q:optSpouseATC_6", Form1701QAtc::Ii017),
                ("frm1701q:optSpouseATC_7", Form1701QAtc::Ii016),
            ],
            self.has_spouse.then_some(self.spouse_atc).flatten(),
        );
        insert(
            &mut fields,
            "frm1701q:txtSpouseName",
            self.spouse_name.clone(),
        );
        insert(
            &mut fields,
            "frm1701q:txtSpouseCitizenship",
            self.spouse_citizenship.clone(),
        );
        insert(
            &mut fields,
            "frm1701q:txtSpouseForeignTaxNum",
            self.spouse_foreign_tax_number.clone(),
        );
        insert_optional_bool_pair(
            &mut fields,
            "frm1701q:optSpouseForeignTaxCred_1",
            "frm1701q:optSpouseForeignTaxCred_2",
            self.has_spouse
                .then_some(self.spouse_claims_foreign_tax_credits)
                .flatten(),
        );
        insert_bool(
            &mut fields,
            "frm1701q:optSpouseTaxRate_1",
            self.has_spouse && self.spouse_tax_rate == Some(Form1701QTaxRate::Graduated),
        );
        insert_bool(
            &mut fields,
            "frm1701q:optSpouseMethod:_1",
            self.has_spouse
                && self.spouse_deduction_method == Some(Form1701QDeductionMethod::Itemized),
        );
        insert_bool(
            &mut fields,
            "frm1701q:optSpouseMethod:_2",
            self.has_spouse && self.spouse_deduction_method == Some(Form1701QDeductionMethod::Osd),
        );
        insert_bool(
            &mut fields,
            "frm1701q:optSpouseTaxRate_2",
            self.has_spouse && self.spouse_tax_rate == Some(Form1701QTaxRate::EightPercent),
        );

        for item in 26..=30 {
            insert_amount_pair(&mut fields, self, item);
        }
        insert_money(
            &mut fields,
            "frm1701q:txt31",
            self.item_31_aggregate_amount_payable.unwrap_or(0.0),
        );
        insert_payment_row(
            &mut fields,
            32,
            &self.payment_details.item_32_cash_or_bank_debit_memo,
            true,
        );
        insert_payment_row(&mut fields, 33, &self.payment_details.item_33_check, true);
        insert_payment_row(
            &mut fields,
            34,
            &self.payment_details.item_34_tax_debit_memo,
            false,
        );
        insert(
            &mut fields,
            "frm1701q:txtParticular35",
            self.payment_details.item_35_others_description.clone(),
        );
        insert_payment_row(&mut fields, 35, &self.payment_details.item_35_others, true);

        insert(&mut fields, "frm1701q:txtPg2TIN1", tin1);
        insert(&mut fields, "frm1701q:txtPg2TIN2", tin2);
        insert(&mut fields, "frm1701q:txtPg2TIN3", tin3);
        insert(&mut fields, "frm1701q:txtPg2BranchCode", branch);
        insert(
            &mut fields,
            "frm1701q:txtPg2TaxpayerName",
            self.taxpayer_last_name.clone(),
        );

        for item in 36..=68 {
            if item == 43 {
                insert(
                    &mut fields,
                    "frm1701q:txt43Desc",
                    self.item_43_non_operating_income_description.clone(),
                );
            }
            if item == 48 {
                insert(
                    &mut fields,
                    "frm1701q:txt48Desc",
                    self.item_48_non_operating_income_description.clone(),
                );
            }
            if item == 61 {
                insert(
                    &mut fields,
                    "frm1701q:txt61Desc",
                    self.item_61_other_tax_credit_description.clone(),
                );
            }
            insert_amount_pair(&mut fields, self, item);
        }

        insert(&mut fields, "frm1701q:txtCurrentPage", "1");
        insert(&mut fields, "frm1701q:txtMaxPage", "2");
        insert(&mut fields, "txtFinalFlag", "0");
        insert(&mut fields, "txtEnroll", "N");
        insert(&mut fields, "ebirOnlineConfirmUsername", "");
        insert(&mut fields, "ebirOnlineUsername", "");
        insert(&mut fields, "ebirOnlineSecret", "");
        insert(
            &mut fields,
            "frm1701q:txtLOB",
            self.line_of_business.clone(),
        );
        insert(
            &mut fields,
            "frm1701q:txtTelno",
            self.contact_number.clone(),
        );
        insert(&mut fields, "driveSelectTPExport", "");

        debug_assert_eq!(fields.len(), OFFICIAL_EDITABLE_FIELD_IDS.len());
        fields
    }

    pub fn to_bir_field_map_checked(
        &self,
    ) -> Result<BTreeMap<String, String>, Vec<(String, String)>> {
        let fields = self.to_bir_field_map();
        let mut errors = self.validate();
        errors.extend(editable_envelope_errors(&fields));
        if errors.is_empty() {
            Ok(fields)
        } else {
            Err(errors)
        }
    }

    /// Generate the exact editable (non-final, non-submission) HTA shape.
    ///
    /// This is official `saveXML(false)`, not Validate. Unset Item 7 / 8 / 16
    /// radios emit as all-false, matching a minimum official Save. Filing-complete
    /// checks stay on [`FormValidator::validate`] and
    /// [`Self::to_bir_field_map_checked`]. Queue submission stays disabled.
    pub fn to_bir_xml_payload(&self) -> Result<String, Vec<(String, String)>> {
        let mut errors = blocked_save_identity_errors(self);
        let fields = self.to_bir_field_map();
        errors.extend(editable_envelope_errors(&fields));
        if !errors.is_empty() {
            return Err(errors);
        }
        let xml = serialize_editable_xml(&fields);
        errors.extend(blocked_save_text_errors(&xml));
        if errors.is_empty() {
            Ok(xml)
        } else {
            Err(errors)
        }
    }

    /// Import an official `saveXML(false)` envelope.
    ///
    /// Unset Item 7 / 8 / 16 radios become `None` on the draft. Malformed
    /// envelopes, unknown keys, credential-bearing fields, and final/outbound
    /// copies still fail closed.
    pub fn from_bir_xml_payload(xml: &str) -> Result<Self, Vec<(String, String)>> {
        let blocked = blocked_save_text_errors(xml);
        if !blocked.is_empty() {
            return Err(blocked);
        }

        let mut errors = Vec::new();
        if !xml.starts_with("<?xml version='1.0'?>") {
            errors.push((
                "xml_payload".to_string(),
                "1701Q editable XML must start with the official declaration".to_string(),
            ));
        }
        if !xml.trim_end().ends_with(EDITABLE_XML_CLOSE) {
            errors.push((
                "xml_payload".to_string(),
                "Only the editable saveXML(false) envelope is supported; final/outbound copies remain disabled"
                    .to_string(),
            ));
        }

        let mut fields = crate::bir_xml::parse_bir_xml_encoded_checked(xml).map_err(|error| {
            vec![(
                "xml_payload".to_string(),
                format!("Invalid 1701Q pseudo-XML: {error}"),
            )]
        })?;
        for field_id in JAVASCRIPT_ESCAPED_FIELDS {
            let Some(encoded) = fields.get(field_id).cloned() else {
                continue;
            };
            match crate::bir_xml::decode_bir_value(&encoded) {
                Some(value) => {
                    fields.insert(field_id.to_string(), value);
                }
                None => errors.push((
                    field_id.to_string(),
                    "Invalid legacy JavaScript escape sequence".to_string(),
                )),
            }
        }
        if !errors.is_empty() {
            return Err(errors);
        }
        let draft = Self::from_bir_field_map(&fields)?;
        let blocked = blocked_save_identity_errors(&draft);
        if blocked.is_empty() {
            Ok(draft)
        } else {
            Err(blocked)
        }
    }

    /// Read an official `saveXML(false)` file as bytes so CRLF survives.
    ///
    /// The filename does not have to match
    /// [`Self::default_submission_filename`]. Queue submission stays disabled.
    pub fn from_bir_xml_bytes(bytes: &[u8]) -> Result<Self, Vec<(String, String)>> {
        let xml = std::str::from_utf8(bytes).map_err(|_| {
            vec![(
                "xml_file".to_string(),
                "Official Save must be UTF-8".to_string(),
            )]
        })?;
        Self::from_bir_xml_payload(xml)
    }

    pub fn from_bir_xml_file(path: impl AsRef<Path>) -> Result<Self, Vec<(String, String)>> {
        let path = path.as_ref();
        if path_looks_blocked(path) {
            return Err(blocked_save_tin_errors("xml_file"));
        }
        let bytes = std::fs::read(path).map_err(|_| {
            vec![(
                "xml_file".to_string(),
                "Could not read official Save file".to_string(),
            )]
        })?;
        Self::from_bir_xml_bytes(&bytes)
    }

    /// Write official `saveXML(false)` bytes (`fs::write`, not text mode).
    ///
    /// Filing `validate()` is not required. Envelope checks and the blocked-TIN
    /// deny-list still fail closed.
    pub fn write_bir_xml_file(&self, path: impl AsRef<Path>) -> Result<(), Vec<(String, String)>> {
        let path = path.as_ref();
        if path_looks_blocked(path) {
            return Err(blocked_save_tin_errors("xml_file"));
        }
        let xml = self.to_bir_xml_payload()?;
        std::fs::write(path, xml.as_bytes()).map_err(|_| {
            vec![(
                "xml_file".to_string(),
                "Could not write official Save file".to_string(),
            )]
        })
    }

    /// Import replaces the open editor only when the Save TIN matches.
    /// Year and quarter may differ; the imported period becomes the draft slot.
    pub fn reject_unless_same_tin(&self, open_tin: &str) -> Result<(), Vec<(String, String)>> {
        if tin_digits(&self.tin) == tin_digits(open_tin) {
            Ok(())
        } else {
            Err(vec![(
                "tin".to_string(),
                "Official Save TIN does not match the open 1701Q draft".to_string(),
            )])
        }
    }

    pub fn from_bir_field_map(
        fields: &BTreeMap<String, String>,
    ) -> Result<Self, Vec<(String, String)>> {
        let mut errors = validate_exact_keys(fields);
        validate_editable_metadata(fields, &mut errors);

        let taxable_year = parse_required::<u16>(fields, "frm1701q:txtYear", &mut errors);
        let quarter = parse_one_of(
            fields,
            &[
                ("frm1701q:DateQuarter_1", 1_u8),
                ("frm1701q:DateQuarter_2", 2_u8),
                ("frm1701q:DateQuarter_3", 3_u8),
            ],
            true,
            "quarter",
            &mut errors,
        );
        let is_amended = parse_required_bool_pair(
            fields,
            "frm1701q:AmendedRtn_1",
            "frm1701q:AmendedRtn_2",
            "is_amended",
            &mut errors,
        );
        let number_of_sheets = parse_required::<u8>(fields, "frm1701q:txtSheets", &mut errors);
        // Official Save preflight does not require Item 7 / 8 / 16. A live
        // minimum dummy Save writes every optType_*, optATC_*, and optTaxRate_*
        // flag as false. Conflicting true values still fail closed.
        let filer_type = parse_one_of(
            fields,
            &[
                ("frm1701q:optType_1", Form1701QFilerType::SingleProprietor),
                ("frm1701q:optType_2", Form1701QFilerType::Professional),
                ("frm1701q:optType_3", Form1701QFilerType::Estate),
                ("frm1701q:optType_4", Form1701QFilerType::Trust),
            ],
            false,
            "filer_type",
            &mut errors,
        );
        let atc = parse_one_of(
            fields,
            &[
                ("frm1701q:optATC_1", Form1701QAtc::Ii012),
                ("frm1701q:optATC_2", Form1701QAtc::Ii014),
                ("frm1701q:optATC_3", Form1701QAtc::Ii013),
                ("frm1701q:optATC_4", Form1701QAtc::Ii015),
                ("frm1701q:optATC_5", Form1701QAtc::Ii017),
                ("frm1701q:optATC_6", Form1701QAtc::Ii016),
            ],
            false,
            "atc",
            &mut errors,
        );
        let claims_foreign_tax_credits = parse_optional_bool_pair(
            fields,
            "frm1701q:optForeignTaxCredits_1",
            "frm1701q:optForeignTaxCredits_2",
            "claims_foreign_tax_credits",
            &mut errors,
        );
        let tax_rate = parse_one_of(
            fields,
            &[
                ("frm1701q:optTaxRate_1", Form1701QTaxRate::Graduated),
                ("frm1701q:optTaxRate_2", Form1701QTaxRate::EightPercent),
            ],
            false,
            "tax_rate",
            &mut errors,
        );
        let deduction_method = parse_one_of(
            fields,
            &[
                (
                    "frm1701q:optMethodOfDeduction:_1",
                    Form1701QDeductionMethod::Itemized,
                ),
                (
                    "frm1701q:optMethodOfDeduction:_2",
                    Form1701QDeductionMethod::Osd,
                ),
            ],
            false,
            "deduction_method",
            &mut errors,
        );

        let spouse_type = parse_one_of(
            fields,
            &[
                (
                    "frm1701q:optSpouseType_1",
                    Form1701QSpouseType::SingleProprietor,
                ),
                (
                    "frm1701q:optSpouseType_2",
                    Form1701QSpouseType::Professional,
                ),
                (
                    "frm1701q:optSpouseType_3",
                    Form1701QSpouseType::CompensationEarner,
                ),
            ],
            false,
            "spouse_type",
            &mut errors,
        );
        let spouse_atc = parse_one_of(
            fields,
            &[
                ("frm1701q:optSpouseATC_1", Form1701QAtc::Ii012),
                ("frm1701q:optSpouseATC_2", Form1701QAtc::Ii014),
                ("frm1701q:optSpouseATC_3", Form1701QAtc::Ii013),
                ("frm1701q:optSpouseATC_4", Form1701QAtc::Ii011),
                ("frm1701q:optSpouseATC_5", Form1701QAtc::Ii015),
                ("frm1701q:optSpouseATC_6", Form1701QAtc::Ii017),
                ("frm1701q:optSpouseATC_7", Form1701QAtc::Ii016),
            ],
            false,
            "spouse_atc",
            &mut errors,
        );
        let spouse_claims_foreign_tax_credits = parse_optional_bool_pair(
            fields,
            "frm1701q:optSpouseForeignTaxCred_1",
            "frm1701q:optSpouseForeignTaxCred_2",
            "spouse_claims_foreign_tax_credits",
            &mut errors,
        );
        let spouse_tax_rate = parse_one_of(
            fields,
            &[
                ("frm1701q:optSpouseTaxRate_1", Form1701QTaxRate::Graduated),
                (
                    "frm1701q:optSpouseTaxRate_2",
                    Form1701QTaxRate::EightPercent,
                ),
            ],
            false,
            "spouse_tax_rate",
            &mut errors,
        );
        let spouse_deduction_method = parse_one_of(
            fields,
            &[
                (
                    "frm1701q:optSpouseMethod:_1",
                    Form1701QDeductionMethod::Itemized,
                ),
                ("frm1701q:optSpouseMethod:_2", Form1701QDeductionMethod::Osd),
            ],
            false,
            "spouse_deduction_method",
            &mut errors,
        );

        let mut source_amounts = BTreeMap::new();
        for item in PAIRED_AMOUNT_ITEMS {
            let taxpayer = parse_money(fields, &format!("frm1701q:txt{item}A"), &mut errors);
            let spouse = parse_money(fields, &format!("frm1701q:txt{item}B"), &mut errors);
            source_amounts.insert(*item, (taxpayer, spouse));
        }
        let source_item_31 = parse_money(fields, "frm1701q:txt31", &mut errors);

        let spouse_identity_present = [
            "frm1701q:txtSpouseTIN1",
            "frm1701q:txtSpouseTIN2",
            "frm1701q:txtSpouseTIN3",
            "frm1701q:txtSpouseBranchCode",
            "frm1701q:txtSpouseName",
            "frm1701q:txtSpouseCitizenship",
            "frm1701q:txtSpouseForeignTaxNum",
        ]
        .iter()
        .any(|key| !field(fields, key).trim().is_empty());
        let spouse_amount_present = source_amounts
            .values()
            .any(|(_, spouse)| spouse.is_some_and(|value| value.abs() > 0.000_001));
        let has_spouse = spouse_identity_present
            || spouse_amount_present
            || spouse_type.is_some()
            || spouse_atc.is_some()
            || spouse_claims_foreign_tax_credits.is_some()
            || spouse_tax_rate.is_some()
            || spouse_deduction_method.is_some();

        verify_repeated_identity(fields, &mut errors);
        if !errors.is_empty() {
            return Err(errors);
        }

        let mut draft = Form1701QDraft {
            id: None,
            taxable_year: taxable_year.unwrap_or_default(),
            quarter: quarter.unwrap_or_default(),
            is_amended: is_amended.unwrap_or_default(),
            number_of_sheets: number_of_sheets.unwrap_or_default(),
            tin: joined_tin(fields, ""),
            rdo_code: field(fields, "frm1701q:txtRDOCode").to_string(),
            filer_type,
            atc,
            taxpayer_name: field(fields, "frm1701q:txtTaxpayerName").to_string(),
            taxpayer_last_name: field(fields, "frm1701q:txtPg2TaxpayerName").to_string(),
            registered_address: field(fields, "frm1701q:txtAddress").to_string(),
            registered_address_2: String::new(),
            zip_code: field(fields, "frm1701q:txtZipCode").to_string(),
            date_of_birth: joined_date(fields),
            email: field(fields, "txtEmail").to_string(),
            citizenship: field(fields, "frm1701q:txtCitizenship").to_string(),
            foreign_tax_number: field(fields, "frm1701q:txtForeignTaxNumber").to_string(),
            claims_foreign_tax_credits,
            tax_rate,
            deduction_method,
            contact_number: field(fields, "frm1701q:txtTelno").to_string(),
            line_of_business: field(fields, "frm1701q:txtLOB").to_string(),
            civil_status: None,
            has_spouse,
            spouse_tin: joined_tin(fields, "Spouse"),
            spouse_rdo_code: normalize_empty_rdo(field(fields, "frm1701q:txtSpouseRDOCode")),
            spouse_type,
            spouse_atc,
            spouse_name: field(fields, "frm1701q:txtSpouseName").to_string(),
            spouse_citizenship: field(fields, "frm1701q:txtSpouseCitizenship").to_string(),
            spouse_foreign_tax_number: field(fields, "frm1701q:txtSpouseForeignTaxNum").to_string(),
            spouse_claims_foreign_tax_credits,
            spouse_tax_rate,
            spouse_deduction_method,
            amounts: Form1701QAmounts::default(),
            item_31_aggregate_amount_payable: None,
            item_43_non_operating_income_description: field(fields, "frm1701q:txt43Desc")
                .to_string(),
            item_48_non_operating_income_description: field(fields, "frm1701q:txt48Desc")
                .to_string(),
            item_61_other_tax_credit_description: field(fields, "frm1701q:txt61Desc").to_string(),
            // A version-0 state: the first compute keeps every penalty the
            // savefile carries as the filer's own (form_1701q_official.rs).
            penalty: Default::default(),
            spouse_penalty: Default::default(),
            payment_details: Form1701QPaymentDetails {
                item_32_cash_or_bank_debit_memo: parse_payment_row(fields, 32, true, &mut errors),
                item_33_check: parse_payment_row(fields, 33, true, &mut errors),
                item_34_tax_debit_memo: parse_payment_row(fields, 34, false, &mut errors),
                item_35_others: parse_payment_row(fields, 35, true, &mut errors),
                item_35_others_description: field(fields, "frm1701q:txtParticular35").to_string(),
                machine_validation_or_receipt_details: String::new(),
            },
            total_tax_due: 0.0,
            total_amount_payable: 0.0,
            last_error: None,
            lifecycle: super::queueable::SubmissionLifecycle::default(),
        };

        for item in USER_ENTERED_AMOUNT_ITEMS {
            if let Some((taxpayer, spouse)) = source_amounts.get(item) {
                if taxpayer.is_some_and(|value| value.abs() > 0.000_001) {
                    draft.set_amount(*item, Form1701QParty::Taxpayer, *taxpayer);
                }
                if has_spouse && spouse.is_some_and(|value| value.abs() > 0.000_001) {
                    draft.set_amount(*item, Form1701QParty::Spouse, *spouse);
                }
            }
        }
        draft.recompute();

        for item in DERIVED_AMOUNT_ITEMS {
            if let Some((source_taxpayer, source_spouse)) = source_amounts.get(item) {
                verify_computed_amount(
                    &format!("frm1701q:txt{item}A"),
                    *source_taxpayer,
                    draft.amount(*item, Form1701QParty::Taxpayer),
                    &mut errors,
                );
                verify_computed_amount(
                    &format!("frm1701q:txt{item}B"),
                    *source_spouse,
                    draft.amount(*item, Form1701QParty::Spouse),
                    &mut errors,
                );
            }
        }
        verify_computed_amount(
            "frm1701q:txt31",
            source_item_31,
            draft.item_31_aggregate_amount_payable,
            &mut errors,
        );
        // Official `saveXML(false)` is not Validate. Filing-complete
        // `draft.validate()` stays on the Validate / queue path so a minimum
        // official Save can import as a draft.

        if errors.is_empty() {
            Ok(draft)
        } else {
            Err(errors)
        }
    }
}

fn tin_digits(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_digit())
        .collect()
}

fn blocked_save_tin_message() -> String {
    "Refusing a savefile that contains a blocked TIN".to_string()
}

fn blocked_save_tin_errors(field: &str) -> Vec<(String, String)> {
    vec![(field.to_string(), blocked_save_tin_message())]
}

fn tin_contains_blocked(value: &str) -> bool {
    tin_digits(value).contains(BLOCKED_SAVE_TIN_DIGITS)
}

fn blocked_save_identity_errors(draft: &Form1701QDraft) -> Vec<(String, String)> {
    let mut errors = Vec::new();
    if tin_contains_blocked(&draft.tin) {
        errors.extend(blocked_save_tin_errors("tin"));
    }
    if tin_contains_blocked(&draft.spouse_tin) {
        errors.extend(blocked_save_tin_errors("spouse_tin"));
    }
    errors
}

fn blocked_save_text_errors(text: &str) -> Vec<(String, String)> {
    if text.contains(BLOCKED_SAVE_TIN_DIGITS) || text.contains(BLOCKED_SAVE_TIN_DASHED) {
        blocked_save_tin_errors("tin")
    } else {
        Vec::new()
    }
}

fn path_looks_blocked(path: &Path) -> bool {
    let displayed = path.to_string_lossy();
    displayed.contains(BLOCKED_SAVE_TIN_DIGITS) || displayed.contains(BLOCKED_SAVE_TIN_DASHED)
}

fn serialize_editable_xml(fields: &BTreeMap<String, String>) -> String {
    let mut output = String::from("<?xml version='1.0'?>");
    output.push_str(XML_FORMAT);
    for field_id in OFFICIAL_EDITABLE_FIELD_IDS {
        let value = fields
            .get(*field_id)
            .expect("exact 1701Q field map contains every official ID");
        let encoded = if JAVASCRIPT_ESCAPED_FIELDS.contains(field_id) {
            javascript_escape(value)
        } else {
            value.clone()
        };
        output.push_str("<div>");
        output.push_str(field_id);
        output.push('=');
        output.push_str(&encoded);
        output.push_str(field_id);
        output.push_str("=</div>");
        output.push_str(XML_FORMAT);
    }
    output.push_str(XML_FORMAT);
    output.push_str(EDITABLE_XML_CLOSE);
    output
}

fn javascript_escape(value: &str) -> String {
    let mut output = String::new();
    for unit in value.encode_utf16() {
        if unit <= 0x7f && is_javascript_escape_safe(unit as u8) {
            output.push(char::from(unit as u8));
        } else if unit <= 0xff {
            output.push_str(&format!("%{unit:02X}"));
        } else {
            output.push_str(&format!("%u{unit:04X}"));
        }
    }
    output
}

fn is_javascript_escape_safe(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'@' | b'*' | b'_' | b'+' | b'-' | b'.' | b'/')
}

fn editable_envelope_errors(fields: &BTreeMap<String, String>) -> Vec<(String, String)> {
    let mut errors = Vec::new();
    for (field_id, value) in fields {
        if !JAVASCRIPT_ESCAPED_FIELDS.contains(&field_id.as_str())
            && (value.contains("<div") || value.contains("</div>"))
        {
            errors.push((
                field_id.clone(),
                "The official editable serializer does not escape nested div markup in this field"
                    .to_string(),
            ));
        }
        if value.chars().any(|character| character == '\0') {
            errors.push((
                field_id.clone(),
                "XML fields cannot contain NUL".to_string(),
            ));
        }
    }
    if fields.len() != super::form_1701q::EXACT_EDITABLE_XML_FIELD_COUNT {
        errors.push((
            "xml_contract".to_string(),
            format!(
                "Expected {} exact editable fields, generated {}",
                super::form_1701q::EXACT_EDITABLE_XML_FIELD_COUNT,
                fields.len()
            ),
        ));
    }
    errors
}

fn validate_exact_keys(fields: &BTreeMap<String, String>) -> Vec<(String, String)> {
    let expected = OFFICIAL_EDITABLE_FIELD_IDS
        .iter()
        .map(|field| (*field).to_string())
        .collect::<BTreeSet<_>>();
    let actual = fields.keys().cloned().collect::<BTreeSet<_>>();
    let mut errors = Vec::new();
    for missing in expected.difference(&actual) {
        errors.push((
            missing.clone(),
            format!("Required exact 1701Q editable field {missing} is missing"),
        ));
    }
    for unknown in actual.difference(&expected) {
        errors.push((
            unknown.clone(),
            format!("Unknown field {unknown} is not in the 172-field editable contract"),
        ));
    }
    errors
}

fn validate_editable_metadata(
    fields: &BTreeMap<String, String>,
    errors: &mut Vec<(String, String)>,
) {
    for (key, expected) in [
        ("frm1701q:txtCurrentPage", "1"),
        ("frm1701q:txtMaxPage", "2"),
        ("txtFinalFlag", "0"),
        ("txtEnroll", "N"),
        ("ebirOnlineConfirmUsername", ""),
        ("ebirOnlineUsername", ""),
        ("ebirOnlineSecret", ""),
        ("driveSelectTPExport", ""),
    ] {
        if fields.get(key).is_some_and(|actual| actual != expected) {
            errors.push((
                key.to_string(),
                format!(
                    "Editable 1701Q import requires {key}={expected:?}; final, enrollment, credential, and export-device state is not persisted"
                ),
            ));
        }
    }
}

fn verify_repeated_identity(fields: &BTreeMap<String, String>, errors: &mut Vec<(String, String)>) {
    for (page_1, page_2) in [
        ("frm1701q:txtTIN1", "frm1701q:txtPg2TIN1"),
        ("frm1701q:txtTIN2", "frm1701q:txtPg2TIN2"),
        ("frm1701q:txtTIN3", "frm1701q:txtPg2TIN3"),
        ("frm1701q:txtBranchCode", "frm1701q:txtPg2BranchCode"),
    ] {
        if field(fields, page_1) != field(fields, page_2) {
            errors.push((
                page_2.to_string(),
                format!("Repeated page-2 value must match {page_1}"),
            ));
        }
    }
}

fn parse_required_bool_pair(
    fields: &BTreeMap<String, String>,
    yes_key: &str,
    no_key: &str,
    semantic_field: &str,
    errors: &mut Vec<(String, String)>,
) -> Option<bool> {
    match (
        parse_bool(fields, yes_key, errors),
        parse_bool(fields, no_key, errors),
    ) {
        (Some(true), Some(false)) => Some(true),
        (Some(false), Some(true)) => Some(false),
        (Some(_), Some(_)) => {
            errors.push((
                semantic_field.to_string(),
                "Exactly one paired official flag must be true".to_string(),
            ));
            None
        }
        _ => None,
    }
}

fn parse_optional_bool_pair(
    fields: &BTreeMap<String, String>,
    yes_key: &str,
    no_key: &str,
    semantic_field: &str,
    errors: &mut Vec<(String, String)>,
) -> Option<bool> {
    match (
        parse_bool(fields, yes_key, errors),
        parse_bool(fields, no_key, errors),
    ) {
        (Some(true), Some(false)) => Some(true),
        (Some(false), Some(true)) => Some(false),
        (Some(false), Some(false)) => None,
        (Some(true), Some(true)) => {
            errors.push((
                semantic_field.to_string(),
                "Paired official flags cannot both be true".to_string(),
            ));
            None
        }
        _ => None,
    }
}

fn parse_one_of<T: Copy>(
    fields: &BTreeMap<String, String>,
    choices: &[(&str, T)],
    required: bool,
    semantic_field: &str,
    errors: &mut Vec<(String, String)>,
) -> Option<T> {
    let selected = choices
        .iter()
        .filter_map(|(key, value)| {
            parse_bool(fields, key, errors)
                .filter(|flag| *flag)
                .map(|_| *value)
        })
        .collect::<Vec<_>>();
    match selected.as_slice() {
        [value] => Some(*value),
        [] if !required => None,
        [] => {
            errors.push((
                semantic_field.to_string(),
                "Exactly one official option must be true".to_string(),
            ));
            None
        }
        _ => {
            errors.push((
                semantic_field.to_string(),
                "Official option flags contain conflicting true values".to_string(),
            ));
            None
        }
    }
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
                format!("Official boolean must be lowercase true or false, found {value:?}"),
            ));
            None
        }
        None => None,
    }
}

fn parse_required<T: std::str::FromStr>(
    fields: &BTreeMap<String, String>,
    key: &str,
    errors: &mut Vec<(String, String)>,
) -> Option<T> {
    match fields.get(key).and_then(|value| value.parse::<T>().ok()) {
        Some(value) => Some(value),
        None => {
            errors.push((key.to_string(), format!("Invalid required value for {key}")));
            None
        }
    }
}

fn parse_money(
    fields: &BTreeMap<String, String>,
    key: &str,
    errors: &mut Vec<(String, String)>,
) -> Option<f64> {
    let value = fields.get(key)?;
    let normalized = value.replace(',', "");
    match normalized.parse::<f64>() {
        Ok(value) if value.is_finite() => Some(value),
        _ => {
            errors.push((key.to_string(), format!("Invalid money value {value:?}")));
            None
        }
    }
}

fn parse_optional_money(
    fields: &BTreeMap<String, String>,
    key: &str,
    errors: &mut Vec<(String, String)>,
) -> Option<f64> {
    if field(fields, key).trim().is_empty() {
        None
    } else {
        parse_money(fields, key, errors)
    }
}

fn parse_payment_row(
    fields: &BTreeMap<String, String>,
    item: u8,
    has_agency: bool,
    errors: &mut Vec<(String, String)>,
) -> Form1701QPaymentRow {
    Form1701QPaymentRow {
        drawee_bank_or_agency: if has_agency {
            field(fields, &format!("frm1701q:txtAgency{item}")).to_string()
        } else {
            String::new()
        },
        number: field(fields, &format!("frm1701q:txtNumber{item}")).to_string(),
        date: field(fields, &format!("frm1701q:txtDate{item}")).to_string(),
        amount: parse_optional_money(fields, &format!("frm1701q:txtAmount{item}"), errors),
    }
}

fn verify_computed_amount(
    key: &str,
    source: Option<f64>,
    computed: Option<f64>,
    errors: &mut Vec<(String, String)>,
) {
    if let Some(source) = source {
        let computed = computed.unwrap_or(0.0);
        if (source - computed).abs() >= 0.001 {
            errors.push((
                key.to_string(),
                format!(
                    "Saved computed value {source:.2} does not match Rust-owned value {computed:.2}"
                ),
            ));
        }
    }
}

fn joined_tin(fields: &BTreeMap<String, String>, spouse: &str) -> String {
    format!(
        "{}{}{}{}",
        field(fields, &format!("frm1701q:txt{spouse}TIN1")),
        field(fields, &format!("frm1701q:txt{spouse}TIN2")),
        field(fields, &format!("frm1701q:txt{spouse}TIN3")),
        field(fields, &format!("frm1701q:txt{spouse}BranchCode")),
    )
}

fn joined_date(fields: &BTreeMap<String, String>) -> String {
    let month = field(fields, "frm1701q:txtBirthMonth");
    let day = field(fields, "frm1701q:txtBirthDay");
    let year = field(fields, "frm1701q:txtBirthYear");
    if month.is_empty() && day.is_empty() && year.is_empty() {
        String::new()
    } else {
        format!("{month}/{day}/{year}")
    }
}

fn normalize_empty_rdo(value: &str) -> String {
    if value == "000" {
        String::new()
    } else {
        value.to_string()
    }
}

fn field<'a>(fields: &'a BTreeMap<String, String>, key: &str) -> &'a str {
    fields.get(key).map(String::as_str).unwrap_or_default()
}

fn insert(map: &mut BTreeMap<String, String>, key: &str, value: impl Into<String>) {
    map.insert(key.to_string(), value.into());
}

fn insert_bool(map: &mut BTreeMap<String, String>, key: &str, value: bool) {
    insert(map, key, if value { "true" } else { "false" });
}

fn insert_optional_bool_pair(
    map: &mut BTreeMap<String, String>,
    yes_key: &str,
    no_key: &str,
    value: Option<bool>,
) {
    insert_bool(map, yes_key, value == Some(true));
    insert_bool(map, no_key, value == Some(false));
}

fn insert_choice<T: Copy + PartialEq>(
    map: &mut BTreeMap<String, String>,
    choices: &[(&str, T)],
    selected: Option<T>,
) {
    for (key, value) in choices {
        insert_bool(map, key, selected == Some(*value));
    }
}

fn insert_amount_pair(map: &mut BTreeMap<String, String>, draft: &Form1701QDraft, item: u8) {
    insert_money(
        map,
        &format!("frm1701q:txt{item}A"),
        draft.amount(item, Form1701QParty::Taxpayer).unwrap_or(0.0),
    );
    insert_money(
        map,
        &format!("frm1701q:txt{item}B"),
        draft.amount(item, Form1701QParty::Spouse).unwrap_or(0.0),
    );
}

fn insert_payment_row(
    map: &mut BTreeMap<String, String>,
    item: u8,
    row: &Form1701QPaymentRow,
    has_agency: bool,
) {
    if has_agency {
        insert(
            map,
            &format!("frm1701q:txtAgency{item}"),
            row.drawee_bank_or_agency.clone(),
        );
    }
    insert(
        map,
        &format!("frm1701q:txtNumber{item}"),
        row.number.clone(),
    );
    insert(map, &format!("frm1701q:txtDate{item}"), row.date.clone());
    insert(
        map,
        &format!("frm1701q:txtAmount{item}"),
        row.amount.map(format_money).unwrap_or_default(),
    );
}

fn insert_money(map: &mut BTreeMap<String, String>, key: &str, value: f64) {
    insert(map, key, format_money(value));
}

fn format_money(value: f64) -> String {
    let negative = value.is_sign_negative() && value != 0.0;
    let cents = (value.abs() * 100.0 + 0.000_000_000_01).floor() as u128;
    let integer = cents / 100;
    let fractional = cents % 100;
    let digits = integer.to_string();
    let mut grouped = String::new();
    for (index, character) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(character);
    }
    format!(
        "{}{grouped}.{fractional:02}",
        if negative { "-" } else { "" }
    )
}

fn split_tin(tin: &str) -> (String, String, String, String) {
    let digits = tin
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect::<String>();
    let segment = |start: usize, end: usize| {
        digits
            .get(start..end.min(digits.len()))
            .unwrap_or_default()
            .to_string()
    };
    (
        segment(0, 3),
        segment(3, 6),
        segment(6, 9),
        digits.get(9..).unwrap_or_default().to_string(),
    )
}

fn split_date(value: &str) -> (String, String, String) {
    let parts = value.split('/').collect::<Vec<_>>();
    if parts.len() == 3 {
        (
            parts[0].to_string(),
            parts[1].to_string(),
            parts[2].to_string(),
        )
    } else {
        (String::new(), String::new(), String::new())
    }
}

#[cfg(test)]
mod tests {
    use super::FormValidator;
    use super::*;
    use sha2::{Digest, Sha256};
    use tempfile::tempdir;

    fn valid_draft() -> Form1701QDraft {
        let mut draft = Form1701QDraft {
            taxable_year: 2021,
            quarter: 2,
            tin: "12345678800000".to_string(),
            rdo_code: "018".to_string(),
            filer_type: Some(Form1701QFilerType::SingleProprietor),
            atc: Some(Form1701QAtc::Ii012),
            taxpayer_name: "PEÑA, JUAN".to_string(),
            taxpayer_last_name: "PEÑA".to_string(),
            registered_address: "12 SAMPLE EXTENSION, ".to_string(),
            registered_address_2: "SAMPLE BRGY".to_string(),
            zip_code: "2200".to_string(),
            date_of_birth: "01/15/1990".to_string(),
            email: "juan+1701q@example.com".to_string(),
            citizenship: "FILIPINO".to_string(),
            claims_foreign_tax_credits: Some(false),
            tax_rate: Some(Form1701QTaxRate::Graduated),
            deduction_method: Some(Form1701QDeductionMethod::Osd),
            contact_number: "1234567".to_string(),
            line_of_business: "SOFTWARE / R&D".to_string(),
            ..Default::default()
        };
        draft.set_amount(36, Form1701QParty::Taxpayer, Some(1_000_000.0));
        draft.set_amount(55, Form1701QParty::Taxpayer, Some(10_000.0));
        draft.set_amount(64, Form1701QParty::Taxpayer, Some(1_000.0));
        draft.recompute();
        draft
    }

    /// 2026-08-21 official dummy Save (`00000000000000-1701Qv2018-2026Q1.xml`).
    /// Bytes stay out of git. Point `BUWIZ_1701Q_LIVE_SAVE_XML` at a temp copy.
    const LIVE_DUMMY_SAVE_BYTES: usize = 11877;
    const LIVE_DUMMY_SAVE_SHA256: &str =
        "bf11bbde0f0f01a416d90bffad00c2eda636604259c49f66e33388e26a259ccc";

    /// Shape-only reconstruction of that Save. It is not the live file: live is
    /// 40 bytes larger because profile-owned fields (Item 12 email, and any
    /// other uncommitted values) are not pinned here.
    const RECONSTRUCTED_MINIMUM_SAVE_BYTES: usize = 11837;
    const RECONSTRUCTED_MINIMUM_SAVE_SHA256: &str =
        "ee283cb9a639a2acd78b598ffcb4d3969f62a396ed4a4c6f7e93f8a60e862cde";

    const UNSET_FILING_RADIO_KEYS: [&str; 12] = [
        "frm1701q:optType_1",
        "frm1701q:optType_2",
        "frm1701q:optType_3",
        "frm1701q:optType_4",
        "frm1701q:optATC_1",
        "frm1701q:optATC_2",
        "frm1701q:optATC_3",
        "frm1701q:optATC_4",
        "frm1701q:optATC_5",
        "frm1701q:optATC_6",
        "frm1701q:optTaxRate_1",
        "frm1701q:optTaxRate_2",
    ];

    /// Reconstructs the live 7.9.6.1 dummy Save *shape* (Fill-up + Save only).
    /// Item 7 / 8 / 16 radios are all false. Do not commit the live saveXML.
    /// Do not pin the dummy profile email.
    fn live_dummy_minimum_save_draft() -> Form1701QDraft {
        Form1701QDraft {
            taxable_year: 2026,
            quarter: 1,
            tin: "00000000000000".to_string(),
            rdo_code: "018".to_string(),
            taxpayer_name: "DELA CRUZ JUAN".to_string(),
            taxpayer_last_name: "DELA CRUZ JUAN".to_string(),
            registered_address: "OLONGAPO, ZAMBALES".to_string(),
            zip_code: "2200".to_string(),
            line_of_business: "RETAIL".to_string(),
            ..Default::default()
        }
    }

    fn assert_live_dummy_public_identity(imported: &Form1701QDraft) {
        assert_eq!(imported.tin, "00000000000000");
        assert_eq!(imported.taxpayer_name, "DELA CRUZ JUAN");
        assert!(
            imported.taxpayer_last_name.is_empty()
                || imported.taxpayer_last_name == imported.taxpayer_name,
            "page 2 name is empty or the official copy of Item 9"
        );
        assert_eq!(imported.registered_address, "OLONGAPO, ZAMBALES");
        assert_eq!(imported.zip_code, "2200");
        assert_eq!(imported.rdo_code, "018");
        assert_eq!(imported.line_of_business, "RETAIL");
        assert_eq!(imported.quarter, 1);
        assert_eq!(imported.taxable_year, 2026);
        assert!(!imported.is_amended);
        assert_eq!(imported.number_of_sheets, 0);
        assert_eq!(imported.filer_type, None);
        assert_eq!(imported.atc, None);
        assert_eq!(imported.tax_rate, None);
        assert_eq!(imported.deduction_method, None);
        assert_eq!(imported.claims_foreign_tax_credits, None);
        assert!(imported.date_of_birth.is_empty());
        assert!(imported.citizenship.is_empty());
        assert_eq!(imported.lifecycle.status, crate::forms::FilingStatus::Draft);

        let fields = imported.to_bir_field_map();
        assert_eq!(fields.len(), 172);
        for key in UNSET_FILING_RADIO_KEYS {
            assert_eq!(fields[key], "false", "{key}");
        }
        for (key, value) in [
            ("frm1701q:txtTIN1", "000"),
            ("frm1701q:txtTIN2", "000"),
            ("frm1701q:txtTIN3", "000"),
            ("frm1701q:txtBranchCode", "00000"),
            ("frm1701q:txtPg2TIN1", "000"),
            ("frm1701q:txtPg2TIN2", "000"),
            ("frm1701q:txtPg2TIN3", "000"),
            ("frm1701q:txtPg2BranchCode", "00000"),
            ("frm1701q:DateQuarter_1", "true"),
            ("frm1701q:AmendedRtn_2", "true"),
            ("frm1701q:txtSheets", "0"),
            ("frm1701q:txt26A", "0.00"),
            ("frm1701q:txt31", "0.00"),
            ("txtFinalFlag", "0"),
            ("txtEnroll", "N"),
            ("ebirOnlineSecret", ""),
        ] {
            assert_eq!(fields[key], value, "{key}");
        }

        let filing_errors = imported.validate();
        assert!(filing_errors.iter().any(|(field, _)| field == "filer_type"));
        assert!(filing_errors.iter().any(|(field, _)| field == "atc"));
        assert!(filing_errors.iter().any(|(field, _)| field == "tax_rate"));
        assert!(imported.to_bir_field_map_checked().is_err());
    }

    fn encoded_field_keys_that_differ(left: &str, right: &str) -> Vec<String> {
        let left_fields =
            crate::bir_xml::parse_bir_xml_encoded_checked(left).expect("left envelope must parse");
        let right_fields = crate::bir_xml::parse_bir_xml_encoded_checked(right)
            .expect("right envelope must parse");
        let keys = left_fields
            .keys()
            .chain(right_fields.keys())
            .cloned()
            .collect::<BTreeSet<_>>();
        keys.into_iter()
            .filter(|key| left_fields.get(key) != right_fields.get(key))
            .collect()
    }

    #[test]
    fn exact_editable_inventory_is_locked() {
        assert_eq!(
            OFFICIAL_EDITABLE_FIELD_IDS.len(),
            super::super::form_1701q::EXACT_EDITABLE_XML_FIELD_COUNT
        );
        assert_eq!(
            OFFICIAL_EDITABLE_FIELD_IDS
                .iter()
                .copied()
                .collect::<BTreeSet<_>>()
                .len(),
            OFFICIAL_EDITABLE_FIELD_IDS.len()
        );
        let inventory = format!("{}\n", OFFICIAL_EDITABLE_FIELD_IDS.join("\n"));
        assert_eq!(
            hex::encode(Sha256::digest(inventory.as_bytes())),
            super::super::form_1701q::EXACT_EDITABLE_FIELD_IDS_SHA256
        );
    }

    #[test]
    fn field_map_contains_all_and_only_the_editable_contract() {
        let fields = valid_draft().to_bir_field_map_checked().unwrap();

        assert_eq!(fields.len(), 172);
        assert!(!fields.contains_key("frm1701q:txtAddress2"));
        assert_eq!(
            fields["frm1701q:txtAddress"],
            "12 SAMPLE EXTENSION, SAMPLE BRGY"
        );
        assert_eq!(fields["frm1701q:txt36A"], "1,000,000.00");
        assert_eq!(fields["frm1701q:DateQuarter_2"], "true");
        assert_eq!(fields["frm1701q:txtCurrentPage"], "1");
        assert_eq!(fields["txtFinalFlag"], "0");
        assert_eq!(fields["txtEnroll"], "N");
        assert_eq!(fields["ebirOnlineSecret"], "");
    }

    #[test]
    fn editable_xml_uses_official_order_envelope_and_javascript_escape() {
        let mut draft = valid_draft();
        draft.taxpayer_name = "PEÑA 你好 😀".to_string();
        let xml = draft.to_bir_xml_payload().unwrap();

        assert!(xml.starts_with("<?xml version='1.0'?>\t\r\n            <div>frm1701q:txtYear="));
        assert!(xml.contains("PE%D1A%20%u4F60%u597D%20%uD83D%uDE00"));
        assert!(xml.ends_with(EDITABLE_XML_CLOSE));
        assert!(!xml.ends_with("All Rights Reserved BIR 2012.0"));
        assert!(
            xml.find("frm1701q:txtAddress=").unwrap() < xml.find("frm1701q:txtZipCode=").unwrap()
        );
    }

    #[test]
    fn generated_editable_payload_round_trips_its_exact_field_map() {
        let mut draft = valid_draft();
        draft.item_43_non_operating_income_description = "5% ADJUSTMENT".to_string();
        draft.set_amount(43, Form1701QParty::Taxpayer, Some(1_000.0));
        draft.recompute();
        let original_fields = draft.to_bir_field_map_checked().unwrap();
        let xml = draft.to_bir_xml_payload().unwrap();
        let imported = Form1701QDraft::from_bir_xml_payload(&xml).unwrap();

        assert!(xml.contains("frm1701q:txt43Desc=5% ADJUSTMENT"));
        assert_eq!(imported.to_bir_field_map(), original_fields);
        assert_eq!(imported.lifecycle.status, crate::forms::FilingStatus::Draft);
    }

    #[test]
    fn minimum_official_save_with_unset_radios_imports_as_draft() {
        let source = live_dummy_minimum_save_draft();
        let fields = source.to_bir_field_map();
        let xml = source.to_bir_xml_payload().expect("saveXML(false) emit");
        assert!(xml.starts_with("<?xml version='1.0'?>\t\r\n            <div>frm1701q:txtYear="));
        assert!(xml.contains("DELA%20CRUZ%20JUAN"));
        assert!(xml.contains("OLONGAPO%2C%20ZAMBALES"));
        let imported = Form1701QDraft::from_bir_xml_payload(&xml)
            .expect("minimum official Save must import as a draft");

        assert_live_dummy_public_identity(&imported);
        assert!(imported.email.is_empty());
        assert_eq!(imported.taxpayer_last_name, "DELA CRUZ JUAN");
        assert_eq!(imported.to_bir_field_map(), fields);
        assert!(source.to_bir_field_map_checked().is_err());
    }

    #[test]
    fn conflicting_official_option_flags_fail_closed() {
        let mut fields = live_dummy_minimum_save_draft().to_bir_field_map();
        fields.insert("frm1701q:optType_1".to_string(), "true".to_string());
        fields.insert("frm1701q:optType_2".to_string(), "true".to_string());

        let errors = Form1701QDraft::from_bir_xml_payload(&serialize_editable_xml(&fields))
            .expect_err("conflicting radios must fail closed");
        assert!(
            errors.iter().any(|(field, message)| {
                field == "filer_type" && message.contains("conflicting")
            })
        );
    }

    #[test]
    fn reconstructed_minimum_save_is_not_the_hash_pinned_live_file() {
        let xml = live_dummy_minimum_save_draft()
            .to_bir_xml_payload()
            .expect("saveXML(false) emit");
        let digest = hex::encode(Sha256::digest(xml.as_bytes()));
        assert_eq!(xml.len(), RECONSTRUCTED_MINIMUM_SAVE_BYTES);
        assert_eq!(digest, RECONSTRUCTED_MINIMUM_SAVE_SHA256);
        assert_ne!(digest, LIVE_DUMMY_SAVE_SHA256);
        assert_ne!(xml.len(), LIVE_DUMMY_SAVE_BYTES);

        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("reconstructed-minimum.xml");
        std::fs::write(&path, xml.as_bytes()).expect("write reconstructed Save bytes");
        let imported = Form1701QDraft::from_bir_xml_file(&path)
            .expect("reconstructed minimum Save must import via from_bir_xml_file");
        assert_live_dummy_public_identity(&imported);
        assert!(imported.email.is_empty());
    }

    #[test]
    fn live_dummy_save_xml_env_imports_without_birforms_exe() {
        // Temp copy of C:\eBIRForms\savefile\00000000000000-1701Qv2018-2026Q1.xml
        // or %TEMP%\buwiz-live-1701q-20260821.xml. Do not commit the XML.
        let Ok(path) = std::env::var("BUWIZ_1701Q_LIVE_SAVE_XML") else {
            return;
        };
        let bytes = std::fs::read(&path).expect("live Save temp copy must be readable");
        assert_eq!(
            bytes.len(),
            LIVE_DUMMY_SAVE_BYTES,
            "BUWIZ_1701Q_LIVE_SAVE_XML is not the 2026-08-21 dummy Save"
        );
        assert_eq!(
            hex::encode(Sha256::digest(&bytes)),
            LIVE_DUMMY_SAVE_SHA256,
            "BUWIZ_1701Q_LIVE_SAVE_XML is not the 2026-08-21 dummy Save"
        );
        assert!(
            bytes.windows(2).any(|pair| pair == b"\r\n"),
            "live Save bytes keep CRLF; compare with read_bytes(), not text mode"
        );
        let xml = std::str::from_utf8(&bytes).expect("editable Save is UTF-8");
        assert!(
            !xml.contains("000123453") && !xml.contains("000-123-453"),
            "do not decode a real TIN savefile"
        );

        let imported = Form1701QDraft::from_bir_xml_file(&path)
            .expect("minimum official Save must import as a draft via from_bir_xml_file");
        let from_bytes = Form1701QDraft::from_bir_xml_bytes(&bytes)
            .expect("minimum official Save must import as a draft via from_bir_xml_bytes");
        assert_eq!(imported.to_bir_field_map(), from_bytes.to_bir_field_map());
        assert_live_dummy_public_identity(&imported);
        assert!(
            imported.email.contains('@'),
            "live dummy Save stamps profile email (Item 12); do not pin the address here"
        );

        let emitted = imported
            .to_bir_xml_payload()
            .expect("saveXML(false) emit must run after import");
        if emitted.as_bytes() != bytes.as_slice() {
            let differing = encoded_field_keys_that_differ(xml, &emitted);
            panic!(
                "import then saveXML(false) emit must match the live dummy Save ({} live bytes vs {} emit bytes); differing field keys: {differing:?}",
                bytes.len(),
                emitted.len()
            );
        }
    }

    #[test]
    fn official_save_file_round_trips_bytes_without_official_filename() {
        let source = live_dummy_minimum_save_draft();
        assert_eq!(
            source.default_submission_filename(),
            "00000000000000-1701Qv2018-2026Q1.xml"
        );

        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("scratch.xml");
        source
            .write_bir_xml_file(&path)
            .expect("dummy Save must write");

        let written = std::fs::read(&path).expect("written Save must be readable");
        assert!(
            written.windows(2).any(|pair| pair == b"\r\n"),
            "official Save bytes keep CRLF"
        );
        assert_eq!(
            written,
            source.to_bir_xml_payload().unwrap().as_bytes(),
            "write_bir_xml_file must persist payload bytes, not a text-mode rewrite"
        );

        let imported = Form1701QDraft::from_bir_xml_file(&path)
            .expect("non-official filename must still import");
        assert_eq!(imported.tin, "00000000000000");
        assert_eq!(imported.taxpayer_name, "DELA CRUZ JUAN");
        assert_eq!(imported.filer_type, None);
        assert_eq!(imported.atc, None);
        assert_eq!(imported.tax_rate, None);
        assert_eq!(imported.to_bir_field_map(), source.to_bir_field_map());
        imported
            .reject_unless_same_tin("000-000-000-00000")
            .expect("dashed dummy TIN matches");
        assert!(imported.reject_unless_same_tin("12345678900000").is_err());
    }

    #[test]
    fn official_save_file_rejects_invalid_utf8() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("scratch.xml");
        std::fs::write(&path, [0xff, 0xfe]).expect("invalid bytes");
        let errors =
            Form1701QDraft::from_bir_xml_file(&path).expect_err("non-UTF-8 Save must fail closed");
        assert!(errors.iter().any(|(field, _)| field == "xml_file"));
    }

    #[test]
    fn blocked_save_tin_is_not_emitted_imported_or_written() {
        let mut blocked = live_dummy_minimum_save_draft();
        blocked.tin = format!("{BLOCKED_SAVE_TIN_DIGITS}00000");
        assert!(blocked.to_bir_xml_payload().is_err());

        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("scratch.xml");
        assert!(blocked.write_bir_xml_file(&path).is_err());
        assert!(!path.exists(), "blocked TIN must not write a Save file");

        let mut fields = live_dummy_minimum_save_draft().to_bir_field_map();
        for (key, value) in [
            ("frm1701q:txtTIN1", &BLOCKED_SAVE_TIN_DIGITS[0..3]),
            ("frm1701q:txtTIN2", &BLOCKED_SAVE_TIN_DIGITS[3..6]),
            ("frm1701q:txtTIN3", &BLOCKED_SAVE_TIN_DIGITS[6..9]),
            ("frm1701q:txtPg2TIN1", &BLOCKED_SAVE_TIN_DIGITS[0..3]),
            ("frm1701q:txtPg2TIN2", &BLOCKED_SAVE_TIN_DIGITS[3..6]),
            ("frm1701q:txtPg2TIN3", &BLOCKED_SAVE_TIN_DIGITS[6..9]),
        ] {
            fields.insert(key.to_string(), value.to_string());
        }
        let xml = serialize_editable_xml(&fields);
        assert!(
            !xml.contains(BLOCKED_SAVE_TIN_DIGITS),
            "split TIN fields must still be caught by identity checks"
        );
        let errors = Form1701QDraft::from_bir_xml_payload(&xml)
            .expect_err("blocked reconstructed TIN must fail closed");
        assert!(errors.iter().any(|(field, _)| field == "tin"));

        let mut named = live_dummy_minimum_save_draft();
        named.registered_address = format!("MARKER {BLOCKED_SAVE_TIN_DASHED}");
        assert!(named.to_bir_xml_payload().is_err());
    }

    #[test]
    fn spouse_and_payment_fields_round_trip_without_enabling_submission() {
        let mut draft = valid_draft();
        draft.has_spouse = true;
        draft.spouse_tin = "98765432100000".to_string();
        draft.spouse_rdo_code = "018".to_string();
        draft.spouse_type = Some(Form1701QSpouseType::Professional);
        draft.spouse_atc = Some(Form1701QAtc::Ii014);
        draft.spouse_name = "MARIA PEÑA".to_string();
        draft.spouse_claims_foreign_tax_credits = Some(false);
        draft.spouse_tax_rate = Some(Form1701QTaxRate::Graduated);
        draft.spouse_deduction_method = Some(Form1701QDeductionMethod::Itemized);
        draft.set_amount(36, Form1701QParty::Spouse, Some(500_000.0));
        draft.set_amount(37, Form1701QParty::Spouse, Some(100_000.0));
        draft.set_amount(39, Form1701QParty::Spouse, Some(50_000.0));
        draft.payment_details.item_33_check = Form1701QPaymentRow {
            drawee_bank_or_agency: "LANDBANK".to_string(),
            number: "12345".to_string(),
            date: "07/19/2026".to_string(),
            amount: Some(1_234.5),
        };
        draft.recompute();

        let fields = draft.to_bir_field_map_checked().unwrap();
        let imported =
            Form1701QDraft::from_bir_xml_payload(&draft.to_bir_xml_payload().unwrap()).unwrap();

        assert_eq!(imported.to_bir_field_map(), fields);
        assert_eq!(imported.payment_details.item_33_check.amount, Some(1_234.5));
    }

    /// Item 52 is restored only where its column may hold it (8%, an ATC
    /// other than II016): a save file with a non-zero 52 elsewhere is
    /// refused, as before item 52 was restored (PR 5 review).
    #[test]
    fn item_52_imports_where_the_regime_allows_it_and_is_refused_elsewhere() {
        let mut draft = valid_draft();
        draft.atc = Some(Form1701QAtc::Ii015);
        draft.tax_rate = Some(Form1701QTaxRate::EightPercent);
        draft.deduction_method = None;
        draft.set_amount(36, Form1701QParty::Taxpayer, None);
        draft.set_amount(47, Form1701QParty::Taxpayer, Some(900_000.0));
        draft.set_amount(52, Form1701QParty::Taxpayer, Some(250_000.0));
        draft.recompute();
        let fields = draft.to_bir_field_map_checked().unwrap();
        assert_eq!(fields["frm1701q:txt52A"], "250,000.00");
        let imported = Form1701QDraft::from_bir_xml_payload(&serialize_editable_xml(&fields))
            .expect("8% with II015 takes item 52");
        assert_eq!(
            imported.amount(52, Form1701QParty::Taxpayer),
            Some(250_000.0)
        );

        for (atc, rate, case) in [
            (Form1701QAtc::Ii016, Form1701QTaxRate::EightPercent, "II016"),
            (
                Form1701QAtc::Ii012,
                Form1701QTaxRate::Graduated,
                "graduated rates",
            ),
        ] {
            let mut other = draft.clone();
            other.atc = Some(atc);
            other.tax_rate = Some(rate);
            other.deduction_method =
                (rate == Form1701QTaxRate::Graduated).then_some(Form1701QDeductionMethod::Osd);
            other.recompute();
            let mut fields = other.to_bir_field_map_checked().unwrap();
            fields.insert("frm1701q:txt52A".to_string(), "250,000.00".to_string());
            let errors = Form1701QDraft::from_bir_xml_payload(&serialize_editable_xml(&fields))
                .expect_err(case);
            assert!(
                errors.iter().any(|(key, message)| key == "frm1701q:txt52A"
                    && message
                        == "Saved computed value 250000.00 does not match Rust-owned value 0.00"),
                "{case}: {errors:?}"
            );
        }
    }

    #[test]
    fn eight_percent_and_signed_input_round_trip_canonically() {
        let mut draft = valid_draft();
        draft.atc = Some(Form1701QAtc::Ii015);
        draft.tax_rate = Some(Form1701QTaxRate::EightPercent);
        draft.deduction_method = None;
        draft.set_amount(36, Form1701QParty::Taxpayer, None);
        draft.set_amount(47, Form1701QParty::Taxpayer, Some(500_000.0));
        draft.set_amount(48, Form1701QParty::Taxpayer, Some(10_000.0));
        draft.item_48_non_operating_income_description = "INTEREST".to_string();
        draft.set_amount(50, Form1701QParty::Taxpayer, Some(-1_000.0));
        draft.recompute();

        let fields = draft.to_bir_field_map_checked().unwrap();
        let imported =
            Form1701QDraft::from_bir_xml_payload(&draft.to_bir_xml_payload().unwrap()).unwrap();

        assert_eq!(fields["frm1701q:txt50A"], "-1,000.00");
        assert_eq!(imported.to_bir_field_map(), fields);
    }

    #[test]
    fn malformed_missing_unknown_and_credential_fields_fail_closed() {
        let draft = valid_draft();
        let mut fields = draft.to_bir_field_map_checked().unwrap();

        fields.remove("frm1701q:txtYear");
        assert!(Form1701QDraft::from_bir_field_map(&fields).is_err());

        let mut fields = draft.to_bir_field_map_checked().unwrap();
        fields.insert("unknown".to_string(), "value".to_string());
        assert!(Form1701QDraft::from_bir_field_map(&fields).is_err());

        let mut fields = draft.to_bir_field_map_checked().unwrap();
        fields.insert(
            "ebirOnlineSecret".to_string(),
            "must-not-persist".to_string(),
        );
        let errors = Form1701QDraft::from_bir_xml_payload(&serialize_editable_xml(&fields))
            .expect_err("credential-bearing source shape must fail closed");
        assert!(errors.iter().any(|(field, _)| field == "ebirOnlineSecret"));

        let final_copy = format!("{}0", draft.to_bir_xml_payload().unwrap());
        let errors = Form1701QDraft::from_bir_xml_payload(&final_copy)
            .expect_err("final/outbound envelopes are outside editable support");
        assert!(errors.iter().any(|(field, _)| field == "xml_payload"));
    }

    #[test]
    fn inconsistent_saved_computed_amount_is_rejected() {
        let draft = valid_draft();
        let mut fields = draft.to_bir_field_map_checked().unwrap();
        fields.insert("frm1701q:txt46A".to_string(), "999,999.00".to_string());

        let errors = Form1701QDraft::from_bir_xml_payload(&serialize_editable_xml(&fields))
            .expect_err("Rust-owned derived amount must win");

        assert!(errors.iter().any(|(field, _)| field == "frm1701q:txt46A"));
    }

    #[test]
    fn checked_in_provenance_matches_rust_capability_boundary() {
        let provenance: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/official-source/1701q-2018-source.json"
        ))
        .unwrap();

        assert_eq!(
            provenance["official_package_evidence"]["package_sha256"],
            super::super::form_1701q::OFFICIAL_PACKAGE_SHA256
        );
        assert_eq!(
            provenance["official_package_evidence"]["source_resources"]["hta_decoded_sha256"],
            super::super::form_1701q::OFFICIAL_HTA_RESOURCE_DECODED_SHA256
        );
        assert_eq!(
            provenance["official_package_evidence"]["editable_saved_xml"]["field_count"],
            super::super::form_1701q::EXACT_EDITABLE_XML_FIELD_COUNT
        );
        assert_eq!(
            provenance["official_package_evidence"]["editable_saved_xml"]["supported_by_rust"],
            super::super::form_1701q::XML_ROUND_TRIP_SUPPORTED
        );
        assert_eq!(
            provenance["official_package_evidence"]["submission_boundary"]["queue_submission_supported"],
            super::super::form_1701q::QUEUE_SUBMISSION_SUPPORTED
        );
        assert_eq!(
            super::super::form_1701q::OFFICIAL_PACKAGE_MANIFEST_RESOURCE_ID
                + super::super::form_1701q::OFFICIAL_HTA_MANIFEST_INDEX,
            super::super::form_1701q::OFFICIAL_HTA_RESOURCE_ID
        );
    }

    #[test]
    #[ignore = "requires EBIRFORMS_PACKAGE_PATH pointing to the reviewed BIRForms.exe"]
    fn locked_external_package_contains_the_exact_reviewed_hta_resource() {
        let path = std::env::var("EBIRFORMS_PACKAGE_PATH")
            .expect("set EBIRFORMS_PACKAGE_PATH to reviewed BIRForms.exe");
        let package = std::fs::read(path).expect("official package must be readable");
        assert_eq!(
            hex::encode(Sha256::digest(&package)),
            super::super::form_1701q::OFFICIAL_PACKAGE_SHA256
        );

        let manifest_start = super::super::form_1701q::OFFICIAL_PACKAGE_MANIFEST_FILE_OFFSET;
        let manifest_end =
            manifest_start + super::super::form_1701q::OFFICIAL_PACKAGE_MANIFEST_SIZE;
        let manifest_bytes = &package[manifest_start..manifest_end];
        assert_eq!(
            hex::encode(Sha256::digest(manifest_bytes)),
            super::super::form_1701q::OFFICIAL_PACKAGE_MANIFEST_SHA256
        );
        let manifest_units = manifest_bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>();
        let manifest = String::from_utf16(&manifest_units).expect("manifest is raw UTF-16LE");
        let manifest_entries = manifest
            .trim_start_matches('\u{feff}')
            .split('|')
            .collect::<Vec<_>>();
        assert_eq!(
            manifest_entries[super::super::form_1701q::OFFICIAL_HTA_MANIFEST_INDEX as usize],
            "forms\\BIR-Form1701Qv2018.hta"
        );
        assert!(manifest_entries.iter().all(|entry| {
            let entry = entry.to_ascii_lowercase();
            !entry.ends_with("encrypt.exe") && !entry.ends_with("cftpsend.exe")
        }));

        let start = super::super::form_1701q::OFFICIAL_HTA_RESOURCE_FILE_OFFSET;
        let end = start + super::super::form_1701q::OFFICIAL_HTA_RESOURCE_DECODED_SIZE;
        let decoded = package[start..end]
            .iter()
            .map(|byte| byte ^ 0xff)
            .collect::<Vec<_>>();
        assert_eq!(
            hex::encode(Sha256::digest(&decoded)),
            super::super::form_1701q::OFFICIAL_HTA_RESOURCE_DECODED_SHA256
        );
        let source = String::from_utf8_lossy(&decoded);
        for anchor in [
            "function saveXML(isFinalCopy)",
            "function saveEncryptedProfile(isFromSubmit)",
            "function sendEmail(sourceElement)",
            "RenameAndSendFile(emailFilePath",
            "frm1701q:txtAddress2",
        ] {
            assert!(
                source.contains(anchor),
                "missing reviewed source anchor {anchor}"
            );
        }

        let tools_start = super::super::form_1701q::OFFICIAL_EBIRTOOLS_RESOURCE_FILE_OFFSET;
        let tools_end =
            tools_start + super::super::form_1701q::OFFICIAL_EBIRTOOLS_RESOURCE_DECODED_SIZE;
        let tools = package[tools_start..tools_end]
            .iter()
            .map(|byte| byte ^ 0xff)
            .collect::<Vec<_>>();
        assert_eq!(
            hex::encode(Sha256::digest(&tools)),
            super::super::form_1701q::OFFICIAL_EBIRTOOLS_RESOURCE_DECODED_SHA256
        );
        let tools_source = String::from_utf8_lossy(&tools);
        for anchor in ["Encrypt.exe", "cFTPSend.exe", "RenameAndSendFile"] {
            assert!(
                tools_source.contains(anchor),
                "missing reviewed transport helper anchor {anchor}"
            );
        }
    }
}
