//! Checked BIR editable-save mapping for exact form `1701v2018`.
//!
//! One reviewed plaintext save proves an 837-key field set. Its encrypted
//! companion proves the same set plus `frm1701:txtPg1I9Address2`; both variants
//! are preserved losslessly. Neither source proves final-flag or submission
//! semantics, so electronic queueing remains disabled in the domain.

use std::collections::BTreeMap;

use super::FormValidator;
use super::form_1701::{
    EXACT_REVIEWED_ENCRYPTED_XML_FIELD_COUNT, EXACT_REVIEWED_XML_FIELD_COUNT,
    EXACT_REVIEWED_XML_VERSION, Form1701AmountPair, Form1701AmountSection, Form1701Atc,
    Form1701CivilStatus, Form1701DeductionMethod, Form1701Draft, Form1701EmployerRow,
    Form1701JointFilingStatus, Form1701NolcoRow, Form1701OverpaymentDisposition, Form1701Party,
    Form1701PaymentDetails, Form1701PaymentRow, Form1701SpecialDeductionRow, Form1701Spouse,
    Form1701SpouseType, Form1701TaxRate, Form1701TaxpayerType, REVIEWED_ENCRYPTED_XML_EXTRA_FIELD,
};

type FieldErrors = Vec<(String, String)>;

impl Form1701Draft {
    /// Semantic field map. For an imported exact save this begins with the
    /// complete source map, retaining all unmodeled Part X/attachment fields.
    pub fn to_bir_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = self.preserved_xml_fields.clone();
        let (tin1, tin2, tin3, branch) = split_tin(&self.tin);

        insert(
            &mut fields,
            "frm1701:txtPg1I1Month",
            self.period_end_month.to_string(),
        );
        insert(
            &mut fields,
            "frm1701:txtPg1I1Year",
            self.taxable_year.to_string(),
        );
        insert_yes_no(
            &mut fields,
            "frm1701:rdoPg1I2AmendedYes",
            "frm1701:rdoPg1I2AmendedNo",
            Some(self.is_amended),
        );
        insert_yes_no(
            &mut fields,
            "frm1701:rdoPg1I3ShortPeriodYes",
            "frm1701:rdoPg1I3ShortPeriodNo",
            Some(self.is_short_period),
        );
        insert_tin(
            &mut fields,
            "frm1701:txtPg1I4",
            &tin1,
            &tin2,
            &tin3,
            &branch,
        );
        insert(
            &mut fields,
            "frm1701:txtPg1I5RDOCode",
            self.rdo_code.clone(),
        );
        insert_choice(
            &mut fields,
            &[
                (
                    "frm1701:rdoPg1I6TaxpayerTypeS",
                    Some(Form1701TaxpayerType::SingleProprietor),
                ),
                (
                    "frm1701:rdoPg1I6TaxpayerTypeP",
                    Some(Form1701TaxpayerType::Professional),
                ),
                (
                    "frm1701:rdoPg1I6TaxpayerTypeE",
                    Some(Form1701TaxpayerType::Estate),
                ),
                (
                    "frm1701:rdoPg1I6TaxpayerTypeT",
                    Some(Form1701TaxpayerType::Trust),
                ),
                (
                    "frm1701:rdoPg1I6TaxpayerTypeC",
                    Some(Form1701TaxpayerType::CompensationEarner),
                ),
            ],
            self.taxpayer_type,
        );
        insert_atc_group(&mut fields, "frm1701:rdoPg1I7ATC_", self.atc);
        insert(
            &mut fields,
            "frm1701:txtPg1I8TaxpayerName",
            self.taxpayer_name.clone(),
        );
        insert(
            &mut fields,
            "frm1701:txtPg1I9Address",
            self.registered_address.clone(),
        );
        insert(
            &mut fields,
            "frm1701:txtPg1I9AZipCode",
            self.zip_code.clone(),
        );
        insert(
            &mut fields,
            "frm1701:txtPg1I10BirthDate",
            self.date_of_birth.clone(),
        );
        insert(&mut fields, "txtEmail", self.email.clone());
        insert(
            &mut fields,
            "frm1701:txtPg1I12Citizenship",
            self.citizenship.clone(),
        );
        insert_yes_no(
            &mut fields,
            "frm1701:rdoPg1I13ForeignTaxCreditsYes",
            "frm1701:rdoPg1I13ForeignTaxCreditsNo",
            self.claims_foreign_tax_credits,
        );
        insert(
            &mut fields,
            "frm1701:txtPg1I14ForeignTaxNumber",
            self.foreign_tax_number.clone(),
        );
        insert(
            &mut fields,
            "frm1701:txtPg1I15TelNum",
            self.contact_number.clone(),
        );
        insert_choice(
            &mut fields,
            &[
                (
                    "frm1701:rdoPg1I16CivilStatusS",
                    Some(Form1701CivilStatus::Single),
                ),
                (
                    "frm1701:rdoPg1I16CivilStatusM",
                    Some(Form1701CivilStatus::Married),
                ),
                (
                    "frm1701:rdoPg1I16CivilStatusLS",
                    Some(Form1701CivilStatus::LegallySeparated),
                ),
                (
                    "frm1701:rdoPg1I16CivilStatusW",
                    Some(Form1701CivilStatus::Widowed),
                ),
            ],
            self.civil_status,
        );
        insert_yes_no(
            &mut fields,
            "frm1701:rdoPg1I17SpouseIncomeYes",
            "frm1701:rdoPg1I17SpouseIncomeNo",
            self.spouse_has_income,
        );
        insert_choice(
            &mut fields,
            &[
                (
                    "frm1701:rdoPg1I18FilingStatusJ",
                    Some(Form1701JointFilingStatus::Joint),
                ),
                (
                    "frm1701:rdoPg1I18FilingStatusS",
                    Some(Form1701JointFilingStatus::Separate),
                ),
            ],
            self.joint_filing_status,
        );
        insert_yes_no(
            &mut fields,
            "frm1701:rdoPg1I19IncomeExemptYes",
            "frm1701:rdoPg1I19IncomeExemptNo",
            self.has_exempt_income,
        );
        insert_yes_no(
            &mut fields,
            "frm1701:rdoPg1I20IncomeSpecialYes",
            "frm1701:rdoPg1I20IncomeSpecialNo",
            self.has_special_rate_income,
        );
        insert_rate_and_deduction(
            &mut fields,
            "frm1701:rdoPg1I21",
            self.tax_rate,
            self.deduction_method,
        );

        for item in 22..=31 {
            let (key_a, key_b) = part_ii_keys(item);
            insert_pair(
                &mut fields,
                key_a,
                key_b,
                self.computations.part_ii.get(&item),
            );
        }
        insert_optional_money(
            &mut fields,
            "frm1701:txtPg1I32AggregateAmtPyble",
            self.computations.part_ii_item_32_aggregate,
        );
        insert_choice(
            &mut fields,
            &[
                (
                    "frm1701:rdoPg1OverpaymentRefund",
                    Form1701OverpaymentDisposition::Refund,
                ),
                (
                    "frm1701:rdoPg1OverpaymentTCC",
                    Form1701OverpaymentDisposition::TaxCreditCertificate,
                ),
                (
                    "frm1701:rdoPg1OverpaymentCarryOver",
                    Form1701OverpaymentDisposition::CarryOver,
                ),
            ],
            self.overpayment_disposition,
        );
        insert(
            &mut fields,
            "frm1701:txtPg1I33NumberOfAttachments",
            self.number_of_attachments
                .map(|value| format!("{value:02}"))
                .unwrap_or_default(),
        );
        insert_payment_row(
            &mut fields,
            34,
            &self.payment_details.item_34_cash_or_bank_debit_memo,
        );
        insert_payment_row(&mut fields, 35, &self.payment_details.item_35_check);
        insert_payment_row(
            &mut fields,
            36,
            &self.payment_details.item_36_tax_debit_memo,
        );
        insert_payment_row(&mut fields, 37, &self.payment_details.item_37_others);
        insert(
            &mut fields,
            "frm1701:txtPg1I37Particular",
            self.payment_details.item_37_others_description.clone(),
        );

        for page in [2, 3, 4] {
            insert_tin(
                &mut fields,
                &format!("frm1701:txtPg{page}"),
                &tin1,
                &tin2,
                &tin3,
                &branch,
            );
            insert(
                &mut fields,
                &format!("frm1701:txtPg{page}TaxpayerName"),
                self.taxpayer_name.clone(),
            );
        }

        insert_spouse(&mut fields, &self.spouse);
        for (index, employer) in self.employers.iter().enumerate() {
            insert_employer(&mut fields, index + 1, employer);
        }
        insert_employer_totals(&mut fields, self);

        for item in 4..=7 {
            insert_named_pair(
                &mut fields,
                &format!("frm1701:txtPg2IShed2_{item}"),
                self.computations.schedule_2.get(&item),
            );
        }
        for item in 8..=25 {
            insert_named_pair(
                &mut fields,
                &format!("frm1701:txtPg2IShed3_{item}"),
                self.computations.schedule_3.get(&item),
            );
        }
        for item in 26..=32 {
            insert_named_pair(
                &mut fields,
                &format!("frm1701:txtPg3IShed3_{item}"),
                self.computations.schedule_3.get(&item),
            );
        }
        for item in [19_u8, 20, 27] {
            insert(
                &mut fields,
                &format!(
                    "frm1701:txtPg{}IShed3_{item}Desc",
                    if item <= 20 { 2 } else { 3 }
                ),
                self.computations
                    .schedule_3_descriptions
                    .get(&item)
                    .cloned()
                    .unwrap_or_default(),
            );
        }

        for item in 1..=16 {
            insert_named_pair(
                &mut fields,
                &format!("frm1701:txtPg3IShed4_{item}"),
                self.computations.schedule_4.get(&item),
            );
        }
        for (index, suffix) in ["17a", "17b", "17c", "17d"].iter().enumerate() {
            insert_named_pair(
                &mut fields,
                &format!("frm1701:txtPg3IShed4_{suffix}"),
                self.computations.schedule_4_item_17.get(index),
            );
        }
        insert(
            &mut fields,
            "frm1701:txtPg3IShed4_17dDesc",
            self.computations.schedule_4_item_17d_description.clone(),
        );
        insert_named_pair(
            &mut fields,
            "frm1701:txtPg3IShed4_18",
            self.computations.schedule_4.get(&18),
        );
        insert_schedule_5(&mut fields, self);
        insert_schedule_6(&mut fields, self);

        for item in 1..=5 {
            insert_named_pair(
                &mut fields,
                &format!("frm1701:txtPg4ISc6_{item}"),
                self.computations.part_vi.get(&item),
            );
        }
        for item in 1..=10 {
            insert_named_pair(
                &mut fields,
                &format!("frm1701:txtPg4IPart7_{item}"),
                self.computations.part_vii.get(&item),
            );
        }
        insert(
            &mut fields,
            "frm1701:txtPg4IPart7_9Specify",
            self.computations.part_vii_item_9_description.clone(),
        );
        for item in 1..=10 {
            insert_named_pair(
                &mut fields,
                &format!("frm1701:txtPg4IPart8_{item}"),
                self.computations.part_viii.get(&item),
            );
        }
        for item in 1..=11 {
            insert_named_pair(
                &mut fields,
                &format!("frm1701:txtPg4IPart9_{item}"),
                self.computations.part_ix.get(&item),
            );
        }
        for item in [2_u8, 3, 4, 6, 7, 8, 9] {
            insert(
                &mut fields,
                &format!("frm1701:txtPg4IPart9_{item}Particulars"),
                self.computations
                    .part_ix_descriptions
                    .get(&item)
                    .cloned()
                    .unwrap_or_default(),
            );
        }

        fields
            .entry("frm1701:txtCurrentPage".to_string())
            .or_insert_with(|| "1".to_string());
        insert(&mut fields, "frm1701:txtMaxPage", "4");
        insert(
            &mut fields,
            "frm1701:txtVersion",
            EXACT_REVIEWED_XML_VERSION,
        );
        fields
    }

    /// Unchecked serialization is useful for deterministic preview/debugging.
    /// Use `try_to_bir_xml_payload` before offering an editable-save export.
    pub fn to_bir_xml_payload(&self) -> String {
        crate::bir_xml::generate_bir_xml(&self.to_bir_field_map())
    }

    pub fn try_to_bir_xml_payload(&self) -> Result<String, FieldErrors> {
        let mut errors = exact_snapshot_errors(self);
        errors.extend(self.validate());
        if errors.is_empty() {
            Ok(self.to_bir_xml_payload())
        } else {
            Err(errors)
        }
    }

    pub fn from_bir_xml_payload(xml: &str) -> Result<Self, FieldErrors> {
        let fields = crate::bir_xml::parse_bir_xml_checked(xml).map_err(|error| {
            vec![(
                "xml_payload".to_string(),
                format!("Invalid 1701 editable-save XML: {error}"),
            )]
        })?;
        Self::from_bir_field_map(&fields)
    }

    pub fn from_bir_field_map(fields: &BTreeMap<String, String>) -> Result<Self, FieldErrors> {
        let mut errors = validate_exact_field_set(fields);
        let taxable_year = parse_required::<u16>(fields, "frm1701:txtPg1I1Year", &mut errors);
        let period_end_month = parse_required::<u8>(fields, "frm1701:txtPg1I1Month", &mut errors);
        let is_amended = parse_required_yes_no(
            fields,
            "frm1701:rdoPg1I2AmendedYes",
            "frm1701:rdoPg1I2AmendedNo",
            "is_amended",
            &mut errors,
        );
        let is_short_period = parse_required_yes_no(
            fields,
            "frm1701:rdoPg1I3ShortPeriodYes",
            "frm1701:rdoPg1I3ShortPeriodNo",
            "is_short_period",
            &mut errors,
        );
        if !errors.is_empty() {
            return Err(errors);
        }

        let mut draft = Form1701Draft {
            tin: join_tin(fields, "frm1701:txtPg1I4"),
            taxable_year: taxable_year.expect("checked above"),
            period_end_month: period_end_month.expect("checked above"),
            is_amended: is_amended.expect("checked above"),
            is_short_period: is_short_period.expect("checked above"),
            rdo_code: text(fields, "frm1701:txtPg1I5RDOCode"),
            taxpayer_type: parse_taxpayer_type(fields, &mut errors),
            atc: parse_atc_group(fields, "frm1701:rdoPg1I7ATC_", "atc", &mut errors),
            taxpayer_name: text(fields, "frm1701:txtPg1I8TaxpayerName"),
            registered_address: text(fields, "frm1701:txtPg1I9Address"),
            zip_code: text(fields, "frm1701:txtPg1I9AZipCode"),
            date_of_birth: text(fields, "frm1701:txtPg1I10BirthDate"),
            email: text(fields, "txtEmail"),
            citizenship: text(fields, "frm1701:txtPg1I12Citizenship"),
            claims_foreign_tax_credits: parse_optional_yes_no(
                fields,
                "frm1701:rdoPg1I13ForeignTaxCreditsYes",
                "frm1701:rdoPg1I13ForeignTaxCreditsNo",
                "claims_foreign_tax_credits",
                &mut errors,
            ),
            foreign_tax_number: text(fields, "frm1701:txtPg1I14ForeignTaxNumber"),
            contact_number: text(fields, "frm1701:txtPg1I15TelNum"),
            civil_status: parse_civil_status(fields, &mut errors),
            spouse_has_income: parse_optional_yes_no(
                fields,
                "frm1701:rdoPg1I17SpouseIncomeYes",
                "frm1701:rdoPg1I17SpouseIncomeNo",
                "spouse_has_income",
                &mut errors,
            ),
            joint_filing_status: parse_joint_status(fields, &mut errors),
            has_exempt_income: parse_optional_yes_no(
                fields,
                "frm1701:rdoPg1I19IncomeExemptYes",
                "frm1701:rdoPg1I19IncomeExemptNo",
                "has_exempt_income",
                &mut errors,
            ),
            has_special_rate_income: parse_optional_yes_no(
                fields,
                "frm1701:rdoPg1I20IncomeSpecialYes",
                "frm1701:rdoPg1I20IncomeSpecialNo",
                "has_special_rate_income",
                &mut errors,
            ),
            number_of_attachments: parse_optional::<u8>(
                fields,
                "frm1701:txtPg1I33NumberOfAttachments",
                &mut errors,
            ),
            overpayment_disposition: parse_overpayment(fields, &mut errors),
            preserved_xml_fields: fields.clone(),
            has_exact_xml_snapshot: true,
            ..Form1701Draft::default()
        };
        (draft.tax_rate, draft.deduction_method) =
            parse_rate_and_deduction(fields, "frm1701:rdoPg1I21", "taxpayer", &mut errors);

        for item in 22..=31 {
            let (key_a, key_b) = part_ii_keys(item);
            let pair = parse_pair(fields, key_a, key_b, &mut errors);
            draft.computations.part_ii.insert(item, pair);
        }
        draft.computations.part_ii_item_32_aggregate =
            parse_optional_money(fields, "frm1701:txtPg1I32AggregateAmtPyble", &mut errors);
        draft.payment_details = parse_payment_details(fields, &mut errors);
        draft.spouse = parse_spouse(fields, &mut errors);
        // Part IV Item 2 as the page, the print and the upload read it
        // (`spouse_rdo_code`); the save file's page default `000` means
        // none was entered (PR 7 review).
        let spouse_rdo = draft.spouse.rdo_code.trim();
        if spouse_rdo != "000" {
            draft.spouse_rdo_code = spouse_rdo.to_string();
        }
        draft.employers =
            std::array::from_fn(|index| parse_employer(fields, index + 1, &mut errors));
        parse_employer_totals(fields, &mut draft, &mut errors);

        for item in 4..=7 {
            draft.computations.schedule_2.insert(
                item,
                parse_named_pair(fields, &format!("frm1701:txtPg2IShed2_{item}"), &mut errors),
            );
        }
        for item in 8..=25 {
            draft.computations.schedule_3.insert(
                item,
                parse_named_pair(fields, &format!("frm1701:txtPg2IShed3_{item}"), &mut errors),
            );
        }
        for item in 26..=32 {
            draft.computations.schedule_3.insert(
                item,
                parse_named_pair(fields, &format!("frm1701:txtPg3IShed3_{item}"), &mut errors),
            );
        }
        for item in [19_u8, 20, 27] {
            draft.computations.schedule_3_descriptions.insert(
                item,
                text(
                    fields,
                    &format!(
                        "frm1701:txtPg{}IShed3_{item}Desc",
                        if item <= 20 { 2 } else { 3 }
                    ),
                ),
            );
        }
        for item in 1..=16 {
            draft.computations.schedule_4.insert(
                item,
                parse_named_pair(fields, &format!("frm1701:txtPg3IShed4_{item}"), &mut errors),
            );
        }
        draft.computations.schedule_4_item_17 = std::array::from_fn(|index| {
            let suffix = ["17a", "17b", "17c", "17d"][index];
            parse_named_pair(
                fields,
                &format!("frm1701:txtPg3IShed4_{suffix}"),
                &mut errors,
            )
        });
        draft.computations.schedule_4_item_17d_description =
            text(fields, "frm1701:txtPg3IShed4_17dDesc");
        draft.computations.schedule_4.insert(
            18,
            parse_named_pair(fields, "frm1701:txtPg3IShed4_18", &mut errors),
        );
        parse_schedule_5(fields, &mut draft, &mut errors);
        parse_schedule_6(fields, &mut draft, &mut errors);

        for item in 1..=5 {
            draft.computations.part_vi.insert(
                item,
                parse_named_pair(fields, &format!("frm1701:txtPg4ISc6_{item}"), &mut errors),
            );
        }
        for item in 1..=10 {
            draft.computations.part_vii.insert(
                item,
                parse_named_pair(fields, &format!("frm1701:txtPg4IPart7_{item}"), &mut errors),
            );
        }
        draft.computations.part_vii_item_9_description =
            text(fields, "frm1701:txtPg4IPart7_9Specify");
        for item in 1..=10 {
            draft.computations.part_viii.insert(
                item,
                parse_named_pair(fields, &format!("frm1701:txtPg4IPart8_{item}"), &mut errors),
            );
        }
        for item in 1..=11 {
            draft.computations.part_ix.insert(
                item,
                parse_named_pair(fields, &format!("frm1701:txtPg4IPart9_{item}"), &mut errors),
            );
        }
        for item in [2_u8, 3, 4, 6, 7, 8, 9] {
            draft.computations.part_ix_descriptions.insert(
                item,
                text(fields, &format!("frm1701:txtPg4IPart9_{item}Particulars")),
            );
        }

        if errors.is_empty() {
            Ok(draft)
        } else {
            Err(errors)
        }
    }
}

fn validate_exact_field_set(fields: &BTreeMap<String, String>) -> FieldErrors {
    let mut errors = Vec::new();
    let is_reviewed_plain = fields.len() == EXACT_REVIEWED_XML_FIELD_COUNT
        && !fields.contains_key(REVIEWED_ENCRYPTED_XML_EXTRA_FIELD);
    let is_reviewed_encrypted = fields.len() == EXACT_REVIEWED_ENCRYPTED_XML_FIELD_COUNT
        && fields.contains_key(REVIEWED_ENCRYPTED_XML_EXTRA_FIELD);
    if !is_reviewed_plain && !is_reviewed_encrypted {
        errors.push((
            "xml_field_count".to_string(),
            format!(
                "Expected the reviewed {EXACT_REVIEWED_XML_FIELD_COUNT}-field plain or \
                 {EXACT_REVIEWED_ENCRYPTED_XML_FIELD_COUNT}-field encrypted 1701v2018 save, \
                 found {} fields",
                fields.len()
            ),
        ));
    }
    if fields.get("frm1701:txtVersion").map(String::as_str) != Some(EXACT_REVIEWED_XML_VERSION) {
        errors.push((
            "xml_version".to_string(),
            format!("Expected exact 1701 XML version {EXACT_REVIEWED_XML_VERSION}"),
        ));
    }
    for key in [
        "frm1701:txtPg1I1Year",
        "frm1701:txtPg1I4TIN1",
        "frm1701:txtPg2IShed3_25A",
        "frm1701:txtPg3IShed6_8D",
        "frm1701:txtPg4IPart9_11B",
        "frm1701:txtPg4mSchedD2_10BTYPE",
        "txtFinalFlag",
    ] {
        if !fields.contains_key(key) {
            errors.push((
                "xml_schema".to_string(),
                format!("Exact 1701v2018 field {key} is missing"),
            ));
        }
    }
    errors
}

fn exact_snapshot_errors(draft: &Form1701Draft) -> FieldErrors {
    if !draft.has_exact_xml_snapshot {
        return vec![(
            "xml_snapshot".to_string(),
            "Checked XML export requires an imported exact 837-field 1701v2018 save snapshot"
                .to_string(),
        )];
    }
    validate_exact_field_set(&draft.preserved_xml_fields)
}

fn insert_spouse(fields: &mut BTreeMap<String, String>, spouse: &Form1701Spouse) {
    let (tin1, tin2, tin3, branch) = split_tin(&spouse.tin);
    insert_tin(fields, "frm1701:txtPg2I1", &tin1, &tin2, &tin3, &branch);
    insert(
        fields,
        "frm1701:txtPg2I2SpouseRDOCode",
        spouse.rdo_code.clone(),
    );
    insert_choice(
        fields,
        &[
            (
                "frm1701:rdoPg2I3SpouseTypeS",
                Some(Form1701SpouseType::SingleProprietor),
            ),
            (
                "frm1701:rdoPg2I3SpouseTypeP",
                Some(Form1701SpouseType::Professional),
            ),
            (
                "frm1701:rdoPg2I3SpouseTypeC",
                Some(Form1701SpouseType::CompensationEarner),
            ),
        ],
        spouse.enabled.then_some(spouse.filer_type).flatten(),
    );
    insert_atc_group(
        fields,
        "frm1701:rdoPg2I4ATC_",
        spouse.enabled.then_some(spouse.atc).flatten(),
    );
    insert(fields, "frm1701:txtPg2I5SpouseName", spouse.name.clone());
    insert(
        fields,
        "frm1701:txtPg2I6TelNum",
        spouse.contact_number.clone(),
    );
    insert(
        fields,
        "frm1701:txtPg2I7Citizenship",
        spouse.citizenship.clone(),
    );
    insert_yes_no(
        fields,
        "frm1701:rdoPg2I8ForeignTaxCreditsYes",
        "frm1701:rdoPg2I8ForeignTaxCreditsNo",
        spouse
            .enabled
            .then_some(spouse.claims_foreign_tax_credits)
            .flatten(),
    );
    insert(
        fields,
        "frm1701:txtPg2I9ForeignTaxNumber",
        spouse.foreign_tax_number.clone(),
    );
    insert_yes_no(
        fields,
        "frm1701:rdoPg2I10IncomeExemptYes",
        "frm1701:rdoPg2I10IncomeExemptNo",
        spouse.enabled.then_some(spouse.has_exempt_income).flatten(),
    );
    insert_yes_no(
        fields,
        "frm1701:rdoPg2I11IncomeSpecialYes",
        "frm1701:rdoPg2I11IncomeSpecialNo",
        spouse
            .enabled
            .then_some(spouse.has_special_rate_income)
            .flatten(),
    );
    insert_rate_and_deduction(
        fields,
        "frm1701:rdoPg2I12",
        spouse.enabled.then_some(spouse.tax_rate).flatten(),
        spouse.enabled.then_some(spouse.deduction_method).flatten(),
    );
}

fn parse_spouse(fields: &BTreeMap<String, String>, errors: &mut FieldErrors) -> Form1701Spouse {
    let filer_type = parse_spouse_type(fields, errors);
    let atc = parse_atc_group(fields, "frm1701:rdoPg2I4ATC_", "spouse_atc", errors);
    let tin = join_tin(fields, "frm1701:txtPg2I1");
    let name = text(fields, "frm1701:txtPg2I5SpouseName");
    let enabled = filer_type.is_some()
        || atc.is_some()
        || !digits(&tin).is_empty()
        || !name.trim().is_empty();
    let (tax_rate, deduction_method) =
        parse_rate_and_deduction(fields, "frm1701:rdoPg2I12", "spouse", errors);
    Form1701Spouse {
        enabled,
        tin,
        rdo_code: text(fields, "frm1701:txtPg2I2SpouseRDOCode"),
        filer_type,
        atc,
        name,
        contact_number: text(fields, "frm1701:txtPg2I6TelNum"),
        citizenship: text(fields, "frm1701:txtPg2I7Citizenship"),
        claims_foreign_tax_credits: parse_optional_yes_no(
            fields,
            "frm1701:rdoPg2I8ForeignTaxCreditsYes",
            "frm1701:rdoPg2I8ForeignTaxCreditsNo",
            "spouse_foreign_tax_credits",
            errors,
        ),
        foreign_tax_number: text(fields, "frm1701:txtPg2I9ForeignTaxNumber"),
        has_exempt_income: parse_optional_yes_no(
            fields,
            "frm1701:rdoPg2I10IncomeExemptYes",
            "frm1701:rdoPg2I10IncomeExemptNo",
            "spouse_exempt_income",
            errors,
        ),
        has_special_rate_income: parse_optional_yes_no(
            fields,
            "frm1701:rdoPg2I11IncomeSpecialYes",
            "frm1701:rdoPg2I11IncomeSpecialNo",
            "spouse_special_income",
            errors,
        ),
        tax_rate,
        deduction_method,
    }
}

fn insert_employer(fields: &mut BTreeMap<String, String>, item: usize, row: &Form1701EmployerRow) {
    let prefix = format!("frm1701:txtPg2IShed{item}a");
    insert_bool(
        fields,
        &format!("frm1701:chkPg2IShed{item}a_{item}Taxpayer"),
        row.owner == Some(Form1701Party::Taxpayer),
    );
    insert_bool(
        fields,
        &format!("frm1701:chkPg2IShed{item}a_{item}Spouse"),
        row.owner == Some(Form1701Party::Spouse),
    );
    insert(
        fields,
        &format!("{prefix}_{item}TPName"),
        if row.owner == Some(Form1701Party::Taxpayer) {
            row.employer_name.clone()
        } else {
            String::new()
        },
    );
    insert(
        fields,
        &format!("{prefix}_{item}SName"),
        if row.owner == Some(Form1701Party::Spouse) {
            row.employer_name.clone()
        } else {
            String::new()
        },
    );
    let (tin1, tin2, tin3, branch) = split_tin(&row.employer_tin);
    insert_tin(fields, &format!("{prefix}_"), &tin1, &tin2, &tin3, &branch);
    insert_optional_money(
        fields,
        &format!("frm1701:txtPg2IShed1c_{item}CI"),
        row.compensation_income,
    );
    insert_optional_money(
        fields,
        &format!("frm1701:txtPg2IShed1c_{item}TW"),
        row.tax_withheld,
    );
}

fn parse_employer(
    fields: &BTreeMap<String, String>,
    item: usize,
    errors: &mut FieldErrors,
) -> Form1701EmployerRow {
    let taxpayer = parse_bool(
        fields,
        &format!("frm1701:chkPg2IShed{item}a_{item}Taxpayer"),
        errors,
    );
    let spouse = parse_bool(
        fields,
        &format!("frm1701:chkPg2IShed{item}a_{item}Spouse"),
        errors,
    );
    let owner = match (taxpayer, spouse) {
        (Some(true), Some(false)) => Some(Form1701Party::Taxpayer),
        (Some(false), Some(true)) => Some(Form1701Party::Spouse),
        (Some(false), Some(false)) => None,
        (Some(true), Some(true)) => {
            errors.push((
                format!("employer_{item}_owner"),
                "Employer row cannot be marked for both taxpayer and spouse".to_string(),
            ));
            None
        }
        _ => None,
    };
    let prefix = format!("frm1701:txtPg2IShed{item}a");
    let taxpayer_name = text(fields, &format!("{prefix}_{item}TPName"));
    let spouse_name = text(fields, &format!("{prefix}_{item}SName"));
    let employer_name = match owner {
        Some(Form1701Party::Taxpayer) => taxpayer_name,
        Some(Form1701Party::Spouse) => spouse_name,
        None if !taxpayer_name.is_empty() && spouse_name.is_empty() => taxpayer_name,
        None if taxpayer_name.is_empty() => spouse_name,
        None => {
            if taxpayer_name != spouse_name {
                errors.push((
                    format!("employer_{item}_name"),
                    "Unselected employer row contains conflicting taxpayer/spouse names"
                        .to_string(),
                ));
            }
            taxpayer_name
        }
    };
    Form1701EmployerRow {
        owner,
        employer_name,
        employer_tin: join_tin(fields, &format!("{prefix}_")),
        compensation_income: parse_optional_money(
            fields,
            &format!("frm1701:txtPg2IShed1c_{item}CI"),
            errors,
        ),
        tax_withheld: parse_optional_money(
            fields,
            &format!("frm1701:txtPg2IShed1c_{item}TW"),
            errors,
        ),
    }
}

fn insert_employer_totals(fields: &mut BTreeMap<String, String>, draft: &Form1701Draft) {
    for (suffix, party) in [
        ("3A", Form1701Party::Taxpayer),
        ("3B", Form1701Party::Spouse),
    ] {
        insert_optional_money(
            fields,
            &format!("frm1701:txtPg2IShed1c_{suffix}CI"),
            draft.amount(Form1701AmountSection::Schedule2, 4, party),
        );
        insert_optional_money(
            fields,
            &format!("frm1701:txtPg2IShed1c_{suffix}TW"),
            draft.amount(Form1701AmountSection::PartVii, 5, party),
        );
    }
}

fn parse_employer_totals(
    fields: &BTreeMap<String, String>,
    draft: &mut Form1701Draft,
    errors: &mut FieldErrors,
) {
    for (suffix, party) in [
        ("3A", Form1701Party::Taxpayer),
        ("3B", Form1701Party::Spouse),
    ] {
        let compensation =
            parse_optional_money(fields, &format!("frm1701:txtPg2IShed1c_{suffix}CI"), errors);
        let withheld =
            parse_optional_money(fields, &format!("frm1701:txtPg2IShed1c_{suffix}TW"), errors);
        draft.set_amount(Form1701AmountSection::Schedule2, 4, party, compensation);
        draft.set_amount(Form1701AmountSection::PartVii, 5, party, withheld);
    }
}

fn insert_schedule_5(fields: &mut BTreeMap<String, String>, draft: &Form1701Draft) {
    for (index, row) in draft.computations.schedule_5_taxpayer.iter().enumerate() {
        insert_special_row(fields, index + 1, row);
    }
    insert_optional_money(
        fields,
        "frm1701:txtPg3IShed5_3",
        draft.computations.schedule_5_total_taxpayer,
    );
    for (index, row) in draft.computations.schedule_5_spouse.iter().enumerate() {
        insert_special_row(fields, index + 4, row);
    }
    insert_optional_money(
        fields,
        "frm1701:txtPg3IShed5_6",
        draft.computations.schedule_5_total_spouse,
    );
}

fn insert_special_row(
    fields: &mut BTreeMap<String, String>,
    item: usize,
    row: &Form1701SpecialDeductionRow,
) {
    insert(
        fields,
        &format!("frm1701:txtPg3IShed5_{item}Desc"),
        row.description.clone(),
    );
    insert(
        fields,
        &format!("frm1701:txtPg3IShed5_{item}Legal"),
        row.legal_basis.clone(),
    );
    insert_optional_money(
        fields,
        &format!("frm1701:txtPg3IShed5_{item}Amt"),
        row.amount,
    );
}

fn parse_schedule_5(
    fields: &BTreeMap<String, String>,
    draft: &mut Form1701Draft,
    errors: &mut FieldErrors,
) {
    draft.computations.schedule_5_taxpayer =
        std::array::from_fn(|index| parse_special_row(fields, index + 1, errors));
    draft.computations.schedule_5_total_taxpayer =
        parse_optional_money(fields, "frm1701:txtPg3IShed5_3", errors);
    draft.computations.schedule_5_spouse =
        std::array::from_fn(|index| parse_special_row(fields, index + 4, errors));
    draft.computations.schedule_5_total_spouse =
        parse_optional_money(fields, "frm1701:txtPg3IShed5_6", errors);
}

fn parse_special_row(
    fields: &BTreeMap<String, String>,
    item: usize,
    errors: &mut FieldErrors,
) -> Form1701SpecialDeductionRow {
    Form1701SpecialDeductionRow {
        description: text(fields, &format!("frm1701:txtPg3IShed5_{item}Desc")),
        legal_basis: text(fields, &format!("frm1701:txtPg3IShed5_{item}Legal")),
        amount: parse_optional_money(fields, &format!("frm1701:txtPg3IShed5_{item}Amt"), errors),
    }
}

fn insert_schedule_6(fields: &mut BTreeMap<String, String>, draft: &Form1701Draft) {
    for item in 1..=3 {
        insert_named_pair(
            fields,
            &format!("frm1701:txtPg3IShed6_{item}"),
            draft.computations.schedule_6_summary.get(&item),
        );
    }
    for (index, row) in draft
        .computations
        .schedule_6_taxpayer_nolco
        .iter()
        .enumerate()
    {
        insert_nolco_row(fields, 3, index + 4, row);
    }
    insert_optional_money(
        fields,
        "frm1701:txtPg3IShed6_8D",
        draft.computations.schedule_6_total_taxpayer,
    );
    for (index, row) in draft
        .computations
        .schedule_6_spouse_nolco
        .iter()
        .enumerate()
    {
        insert_nolco_row(fields, 4, index + 9, row);
    }
    insert_optional_money(
        fields,
        "frm1701:txtPg4IShed6_13D",
        draft.computations.schedule_6_total_spouse,
    );
}

fn insert_nolco_row(
    fields: &mut BTreeMap<String, String>,
    page: usize,
    item: usize,
    row: &Form1701NolcoRow,
) {
    let prefix = format!("frm1701:txtPg{page}IShed6_{item}");
    insert(fields, &format!("{prefix}Year"), row.year_incurred.clone());
    for (suffix, value) in [
        ("A", row.amount),
        ("B", row.applied_previous_years),
        ("C", row.expired),
        ("D", row.applied_current_year),
        ("E", row.unapplied),
    ] {
        insert_optional_money(fields, &format!("{prefix}{suffix}"), value);
    }
}

fn parse_schedule_6(
    fields: &BTreeMap<String, String>,
    draft: &mut Form1701Draft,
    errors: &mut FieldErrors,
) {
    for item in 1..=3 {
        draft.computations.schedule_6_summary.insert(
            item,
            parse_named_pair(fields, &format!("frm1701:txtPg3IShed6_{item}"), errors),
        );
    }
    draft.computations.schedule_6_taxpayer_nolco =
        std::array::from_fn(|index| parse_nolco_row(fields, 3, index + 4, errors));
    draft.computations.schedule_6_total_taxpayer =
        parse_optional_money(fields, "frm1701:txtPg3IShed6_8D", errors);
    draft.computations.schedule_6_spouse_nolco =
        std::array::from_fn(|index| parse_nolco_row(fields, 4, index + 9, errors));
    draft.computations.schedule_6_total_spouse =
        parse_optional_money(fields, "frm1701:txtPg4IShed6_13D", errors);
}

fn parse_nolco_row(
    fields: &BTreeMap<String, String>,
    page: usize,
    item: usize,
    errors: &mut FieldErrors,
) -> Form1701NolcoRow {
    let prefix = format!("frm1701:txtPg{page}IShed6_{item}");
    Form1701NolcoRow {
        year_incurred: text(fields, &format!("{prefix}Year")),
        amount: parse_optional_money(fields, &format!("{prefix}A"), errors),
        applied_previous_years: parse_optional_money(fields, &format!("{prefix}B"), errors),
        expired: parse_optional_money(fields, &format!("{prefix}C"), errors),
        applied_current_year: parse_optional_money(fields, &format!("{prefix}D"), errors),
        unapplied: parse_optional_money(fields, &format!("{prefix}E"), errors),
    }
}

fn insert_payment_row(fields: &mut BTreeMap<String, String>, item: u8, row: &Form1701PaymentRow) {
    // The reviewed save has no drawee/agency field for Item 36 Tax Debit Memo.
    if item != 36 {
        insert(
            fields,
            &format!("frm1701:txtPg1I{item}Agency"),
            row.drawee_bank_or_agency.clone(),
        );
    }
    let number_key = if item == 35 {
        "frm1701:txtPg1I235Number".to_string()
    } else {
        format!("frm1701:txtPg1I{item}Number")
    };
    insert(fields, &number_key, row.number.clone());
    insert(
        fields,
        &format!("frm1701:txtPg1I{item}Date"),
        row.date.clone(),
    );
    insert_optional_money(fields, &format!("frm1701:txtPg1I{item}Amount"), row.amount);
}

fn parse_payment_details(
    fields: &BTreeMap<String, String>,
    errors: &mut FieldErrors,
) -> Form1701PaymentDetails {
    Form1701PaymentDetails {
        item_34_cash_or_bank_debit_memo: parse_payment_row(fields, 34, errors),
        item_35_check: parse_payment_row(fields, 35, errors),
        item_36_tax_debit_memo: parse_payment_row(fields, 36, errors),
        item_37_others: parse_payment_row(fields, 37, errors),
        item_37_others_description: text(fields, "frm1701:txtPg1I37Particular"),
    }
}

fn parse_payment_row(
    fields: &BTreeMap<String, String>,
    item: u8,
    errors: &mut FieldErrors,
) -> Form1701PaymentRow {
    let number_key = if item == 35 {
        "frm1701:txtPg1I235Number".to_string()
    } else {
        format!("frm1701:txtPg1I{item}Number")
    };
    Form1701PaymentRow {
        drawee_bank_or_agency: if item == 36 {
            String::new()
        } else {
            text(fields, &format!("frm1701:txtPg1I{item}Agency"))
        },
        number: text(fields, &number_key),
        date: text(fields, &format!("frm1701:txtPg1I{item}Date")),
        amount: parse_optional_money(fields, &format!("frm1701:txtPg1I{item}Amount"), errors),
    }
}

fn parse_taxpayer_type(
    fields: &BTreeMap<String, String>,
    errors: &mut FieldErrors,
) -> Option<Form1701TaxpayerType> {
    parse_choice(
        fields,
        &[
            (
                "frm1701:rdoPg1I6TaxpayerTypeS",
                Form1701TaxpayerType::SingleProprietor,
            ),
            (
                "frm1701:rdoPg1I6TaxpayerTypeP",
                Form1701TaxpayerType::Professional,
            ),
            (
                "frm1701:rdoPg1I6TaxpayerTypeE",
                Form1701TaxpayerType::Estate,
            ),
            ("frm1701:rdoPg1I6TaxpayerTypeT", Form1701TaxpayerType::Trust),
            (
                "frm1701:rdoPg1I6TaxpayerTypeC",
                Form1701TaxpayerType::CompensationEarner,
            ),
        ],
        "taxpayer_type",
        errors,
    )
}

fn parse_spouse_type(
    fields: &BTreeMap<String, String>,
    errors: &mut FieldErrors,
) -> Option<Form1701SpouseType> {
    parse_choice(
        fields,
        &[
            (
                "frm1701:rdoPg2I3SpouseTypeS",
                Form1701SpouseType::SingleProprietor,
            ),
            (
                "frm1701:rdoPg2I3SpouseTypeP",
                Form1701SpouseType::Professional,
            ),
            (
                "frm1701:rdoPg2I3SpouseTypeC",
                Form1701SpouseType::CompensationEarner,
            ),
        ],
        "spouse_type",
        errors,
    )
}

fn parse_civil_status(
    fields: &BTreeMap<String, String>,
    errors: &mut FieldErrors,
) -> Option<Form1701CivilStatus> {
    parse_choice(
        fields,
        &[
            ("frm1701:rdoPg1I16CivilStatusS", Form1701CivilStatus::Single),
            (
                "frm1701:rdoPg1I16CivilStatusM",
                Form1701CivilStatus::Married,
            ),
            (
                "frm1701:rdoPg1I16CivilStatusLS",
                Form1701CivilStatus::LegallySeparated,
            ),
            (
                "frm1701:rdoPg1I16CivilStatusW",
                Form1701CivilStatus::Widowed,
            ),
        ],
        "civil_status",
        errors,
    )
}

fn parse_joint_status(
    fields: &BTreeMap<String, String>,
    errors: &mut FieldErrors,
) -> Option<Form1701JointFilingStatus> {
    parse_choice(
        fields,
        &[
            (
                "frm1701:rdoPg1I18FilingStatusJ",
                Form1701JointFilingStatus::Joint,
            ),
            (
                "frm1701:rdoPg1I18FilingStatusS",
                Form1701JointFilingStatus::Separate,
            ),
        ],
        "joint_filing_status",
        errors,
    )
}

fn parse_overpayment(
    fields: &BTreeMap<String, String>,
    errors: &mut FieldErrors,
) -> Form1701OverpaymentDisposition {
    parse_choice(
        fields,
        &[
            (
                "frm1701:rdoPg1OverpaymentRefund",
                Form1701OverpaymentDisposition::Refund,
            ),
            (
                "frm1701:rdoPg1OverpaymentTCC",
                Form1701OverpaymentDisposition::TaxCreditCertificate,
            ),
            (
                "frm1701:rdoPg1OverpaymentCarryOver",
                Form1701OverpaymentDisposition::CarryOver,
            ),
        ],
        "overpayment_disposition",
        errors,
    )
    .unwrap_or(Form1701OverpaymentDisposition::None)
}

fn insert_atc_group(
    fields: &mut BTreeMap<String, String>,
    prefix: &str,
    selected: Option<Form1701Atc>,
) {
    for atc in Form1701Atc::ALL {
        insert_bool(
            fields,
            &format!("{prefix}{}", atc.code()),
            selected == Some(atc),
        );
    }
}

fn parse_atc_group(
    fields: &BTreeMap<String, String>,
    prefix: &str,
    field_name: &str,
    errors: &mut FieldErrors,
) -> Option<Form1701Atc> {
    let choices = Form1701Atc::ALL.map(|atc| (format!("{prefix}{}", atc.code()), atc));
    let borrowed = choices
        .iter()
        .map(|(key, value)| (key.as_str(), *value))
        .collect::<Vec<_>>();
    parse_choice(fields, &borrowed, field_name, errors)
}

fn insert_rate_and_deduction(
    fields: &mut BTreeMap<String, String>,
    prefix: &str,
    rate: Option<Form1701TaxRate>,
    deduction: Option<Form1701DeductionMethod>,
) {
    insert_bool(
        fields,
        &format!("{prefix}TaxRateG"),
        rate == Some(Form1701TaxRate::Graduated),
    );
    insert_bool(
        fields,
        &format!("{prefix}TaxRateP"),
        rate == Some(Form1701TaxRate::EightPercent),
    );
    insert_bool(
        fields,
        &format!("{prefix}AMethodDeductionI"),
        deduction == Some(Form1701DeductionMethod::Itemized),
    );
    insert_bool(
        fields,
        &format!("{prefix}AMethodDeductionO"),
        deduction == Some(Form1701DeductionMethod::Osd),
    );
}

fn parse_rate_and_deduction(
    fields: &BTreeMap<String, String>,
    prefix: &str,
    field_name: &str,
    errors: &mut FieldErrors,
) -> (Option<Form1701TaxRate>, Option<Form1701DeductionMethod>) {
    let rate = parse_choice(
        fields,
        &[
            (&*format!("{prefix}TaxRateG"), Form1701TaxRate::Graduated),
            (&*format!("{prefix}TaxRateP"), Form1701TaxRate::EightPercent),
        ],
        &format!("{field_name}_tax_rate"),
        errors,
    );
    let deduction = parse_choice(
        fields,
        &[
            (
                &*format!("{prefix}AMethodDeductionI"),
                Form1701DeductionMethod::Itemized,
            ),
            (
                &*format!("{prefix}AMethodDeductionO"),
                Form1701DeductionMethod::Osd,
            ),
        ],
        &format!("{field_name}_deduction_method"),
        errors,
    );
    (rate, deduction)
}

fn part_ii_keys(item: u8) -> (&'static str, &'static str) {
    match item {
        22 => ("frm1701:txtPg1I22ATaxDue", "frm1701:txtPg1I22BTaxDue"),
        23 => ("frm1701:txtPg1I23A", "frm1701:txtPg1I23B"),
        24 => (
            "frm1701:txtPg1I24ATaxPayable",
            "frm1701:txtPg1I24BTaxPayable",
        ),
        25 => ("frm1701:txtPg1I25A", "frm1701:txtPg1I25B"),
        26 => ("frm1701:txtPg1I26A", "frm1701:txtPg1I26B"),
        27 => ("frm1701:txtPg1I27A", "frm1701:txtPg1I27B"),
        28 => ("frm1701:txtPg1I28A", "frm1701:txtPg1I28B"),
        29 => ("frm1701:txtPg1I29A", "frm1701:txtPg1I29B"),
        30 => ("frm1701:txtPg1I30A", "frm1701:txtPg1I30B"),
        31 => (
            "frm1701:txtPg1I31ATotalAmtPyble",
            "frm1701:txtPg1I31BTotalAmtPyble",
        ),
        _ => unreachable!("Part II item outside 22-31"),
    }
}

fn insert_named_pair(
    fields: &mut BTreeMap<String, String>,
    prefix: &str,
    pair: Option<&Form1701AmountPair>,
) {
    insert_pair(fields, &format!("{prefix}A"), &format!("{prefix}B"), pair);
}

fn parse_named_pair(
    fields: &BTreeMap<String, String>,
    prefix: &str,
    errors: &mut FieldErrors,
) -> Form1701AmountPair {
    parse_pair(fields, &format!("{prefix}A"), &format!("{prefix}B"), errors)
}

fn insert_pair(
    fields: &mut BTreeMap<String, String>,
    key_a: &str,
    key_b: &str,
    pair: Option<&Form1701AmountPair>,
) {
    insert_optional_money(fields, key_a, pair.and_then(|pair| pair.taxpayer));
    insert_optional_money(fields, key_b, pair.and_then(|pair| pair.spouse));
}

fn parse_pair(
    fields: &BTreeMap<String, String>,
    key_a: &str,
    key_b: &str,
    errors: &mut FieldErrors,
) -> Form1701AmountPair {
    Form1701AmountPair {
        taxpayer: parse_optional_money(fields, key_a, errors),
        spouse: parse_optional_money(fields, key_b, errors),
    }
}

fn insert_tin(
    fields: &mut BTreeMap<String, String>,
    prefix: &str,
    tin1: &str,
    tin2: &str,
    tin3: &str,
    branch: &str,
) {
    insert(fields, &format!("{prefix}TIN1"), tin1);
    insert(fields, &format!("{prefix}TIN2"), tin2);
    insert(fields, &format!("{prefix}TIN3"), tin3);
    insert(fields, &format!("{prefix}BranchCode"), branch);
}

fn split_tin(tin: &str) -> (String, String, String, String) {
    let digits = digits(tin);
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
        segment(9, digits.len()),
    )
}

fn join_tin(fields: &BTreeMap<String, String>, prefix: &str) -> String {
    ["TIN1", "TIN2", "TIN3", "BranchCode"]
        .iter()
        .map(|suffix| text(fields, &format!("{prefix}{suffix}")))
        .collect()
}

fn digits(value: &str) -> String {
    value.chars().filter(|ch| ch.is_ascii_digit()).collect()
}

fn insert_choice<T: Copy + PartialEq>(
    fields: &mut BTreeMap<String, String>,
    choices: &[(&str, T)],
    selected: T,
) {
    for (key, value) in choices {
        insert_bool(fields, key, selected == *value);
    }
}

fn parse_choice<T: Copy>(
    fields: &BTreeMap<String, String>,
    choices: &[(&str, T)],
    field_name: &str,
    errors: &mut FieldErrors,
) -> Option<T> {
    let selected = choices
        .iter()
        .filter_map(|(key, value)| {
            parse_bool(fields, key, errors)
                .is_some_and(|set| set)
                .then_some(*value)
        })
        .collect::<Vec<_>>();
    match selected.as_slice() {
        [] => None,
        [value] => Some(*value),
        _ => {
            errors.push((
                field_name.to_string(),
                "More than one mutually-exclusive XML choice is selected".to_string(),
            ));
            None
        }
    }
}

fn insert_yes_no(
    fields: &mut BTreeMap<String, String>,
    yes_key: &str,
    no_key: &str,
    value: Option<bool>,
) {
    insert_bool(fields, yes_key, value == Some(true));
    insert_bool(fields, no_key, value == Some(false));
}

fn parse_required_yes_no(
    fields: &BTreeMap<String, String>,
    yes_key: &str,
    no_key: &str,
    field_name: &str,
    errors: &mut FieldErrors,
) -> Option<bool> {
    let result = parse_optional_yes_no(fields, yes_key, no_key, field_name, errors);
    if result.is_none() {
        errors.push((
            field_name.to_string(),
            "XML must select exactly one Yes/No value".to_string(),
        ));
    }
    result
}

fn parse_optional_yes_no(
    fields: &BTreeMap<String, String>,
    yes_key: &str,
    no_key: &str,
    field_name: &str,
    errors: &mut FieldErrors,
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
                field_name.to_string(),
                "XML Yes and No values cannot both be true".to_string(),
            ));
            None
        }
        _ => None,
    }
}

fn parse_bool(
    fields: &BTreeMap<String, String>,
    key: &str,
    errors: &mut FieldErrors,
) -> Option<bool> {
    match fields
        .get(key)
        .map(|value| value.trim().to_ascii_lowercase())
    {
        Some(value) if value == "true" => Some(true),
        Some(value) if value == "false" => Some(false),
        Some(value) => {
            errors.push((
                key.to_string(),
                format!("Expected true/false, found {value:?}"),
            ));
            None
        }
        None => {
            errors.push((key.to_string(), "Required XML field is missing".to_string()));
            None
        }
    }
}

fn parse_required<T: std::str::FromStr>(
    fields: &BTreeMap<String, String>,
    key: &str,
    errors: &mut FieldErrors,
) -> Option<T> {
    let value = fields
        .get(key)
        .map(String::as_str)
        .unwrap_or_default()
        .trim();
    if value.is_empty() {
        errors.push((key.to_string(), "Required XML value is blank".to_string()));
        return None;
    }
    value.parse::<T>().map_err(|_| ()).map_or_else(
        |_| {
            errors.push((key.to_string(), format!("Invalid value {value:?}")));
            None
        },
        Some,
    )
}

fn parse_optional<T: std::str::FromStr>(
    fields: &BTreeMap<String, String>,
    key: &str,
    errors: &mut FieldErrors,
) -> Option<T> {
    let value = fields
        .get(key)
        .map(String::as_str)
        .unwrap_or_default()
        .trim();
    if value.is_empty() {
        return None;
    }
    value.parse::<T>().map_err(|_| ()).map_or_else(
        |_| {
            errors.push((key.to_string(), format!("Invalid value {value:?}")));
            None
        },
        Some,
    )
}

fn parse_optional_money(
    fields: &BTreeMap<String, String>,
    key: &str,
    errors: &mut FieldErrors,
) -> Option<f64> {
    let value = fields
        .get(key)
        .map(String::as_str)
        .unwrap_or_default()
        .trim();
    if value.is_empty() {
        return None;
    }
    match value.replace(',', "").parse::<f64>() {
        Ok(parsed) if parsed.is_finite() => Some(parsed),
        _ => {
            errors.push((key.to_string(), format!("Invalid finite amount {value:?}")));
            None
        }
    }
}

fn text(fields: &BTreeMap<String, String>, key: &str) -> String {
    fields.get(key).cloned().unwrap_or_default()
}

fn insert(fields: &mut BTreeMap<String, String>, key: &str, value: impl Into<String>) {
    fields.insert(key.to_string(), value.into());
}

fn insert_bool(fields: &mut BTreeMap<String, String>, key: &str, value: bool) {
    insert(fields, key, if value { "true" } else { "false" });
}

fn insert_optional_money(fields: &mut BTreeMap<String, String>, key: &str, value: Option<f64>) {
    insert(
        fields,
        key,
        value.map(|value| format!("{value:.2}")).unwrap_or_default(),
    );
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn amended_yes_no_are_true_inverses() {
        let draft = Form1701Draft {
            is_amended: true,
            ..Form1701Draft::default()
        };
        let fields = draft.to_bir_field_map();
        assert_eq!(fields["frm1701:rdoPg1I2AmendedYes"], "true");
        assert_eq!(fields["frm1701:rdoPg1I2AmendedNo"], "false");
    }

    #[test]
    fn blank_and_zero_money_serialize_differently() {
        let mut draft = Form1701Draft::default();
        draft.set_amount(
            Form1701AmountSection::PartVii,
            1,
            Form1701Party::Taxpayer,
            Some(0.0),
        );
        let fields = draft.to_bir_field_map();
        assert_eq!(fields["frm1701:txtPg4IPart7_1A"], "0.00");
        assert_eq!(fields["frm1701:txtPg4IPart7_1B"], "");
    }

    #[test]
    fn checked_export_requires_exact_imported_snapshot() {
        let error = Form1701Draft::default()
            .try_to_bir_xml_payload()
            .expect_err("a new local draft has no complete schema snapshot");
        assert!(error.iter().any(|(field, _)| field == "xml_snapshot"));
    }
}
