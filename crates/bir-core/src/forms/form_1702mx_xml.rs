//! Checked 210-field editable-save contract for exact form `1702MXv2018C`.
//!
//! The plain source XML is authoritative for editable-save persistence. The
//! encrypted companion is hash-locked only: its encryption and submission
//! semantics have not been reviewed. Unknown, missing, and duplicate fields
//! fail closed, while every reviewed raw lexeme is retained for round-trip.

use std::collections::{BTreeMap, BTreeSet};

use super::FormValidator;
use super::form_1702mx::{
    Form1702MXAtcSelection, Form1702MXDeductionMethod, Form1702MXDraft, Form1702MXFilingBasis,
    Form1702MXMandatoryAttachment, Form1702MXOverpaymentDisposition, Form1702MXPartII,
    Form1702MXPaymentDetail, PercentInput, WholePeso, WholePesoInput,
};

const EXACT_SOURCE_FIELD_COUNT: usize = 210;

const EXPECTED_SOURCE_KEYS: [&str; EXACT_SOURCE_FIELD_COUNT] = [
    "frm1702MX:rdoPg1I1Calendar",
    "frm1702MX:rdoPg1I1Fiscal",
    "frm1702MX:ddlPg1I2Date",
    "frm1702MX:txtPg1I2YearEnd",
    "frm1702MX:rdoPg1I3AmendedYes",
    "frm1702MX:rdoPg1I3AmendedNo",
    "frm1702MX:rdoPg1I4ShortPeriodYes",
    "frm1702MX:rdoPg1I4ShortPeriodNo",
    "frm1702MX:chkPg1I5ATCR1",
    "frm1702MX:drpPg1I5ATCR2",
    "frm1702MX:chkPg1I5ATCR2",
    "frm1702MX:txtPg1Pt1I6TINC1",
    "frm1702MX:txtPg1Pt1I6TINC2",
    "frm1702MX:txtPg1Pt1I6TINC3",
    "frm1702MX:txtPg1Pt1I6TINC4",
    "frm1702MX:txtPg1TINMASK",
    "frm1702MX:txtPg1Pt1I7RDO",
    "frm1702MX:drpPg1Pt1I7RDO",
    "frm1702MX:txtPg1Pt1I9RegisteredName",
    "frm1702MX:txtPg1Pt1I9RegisteredName2",
    "frm1702MX:txtPg1Pt1I9RegisteredName3",
    "frm1702MX:txtPg1Pt1I10RegisteredAddress",
    "frm1702MX:txtPg1Pt1I10RegisteredAddress2",
    "frm1702MX:txtPg1Pt1I10RegisteredAddress3",
    "frm1702MX:txtZIP",
    "frm1702MX:txtPg1Pt1I8",
    "frm1702MX:txtPg1Pt1I11ContactNumber",
    "frm1702MX:txtPg1Pt1I12Email",
    "frm1702MX:rdoPg1Pt1I13MethodOfDeducItemized",
    "frm1702MX:rdoPg1Pt1I13MethodOfDeducOptional",
    "frm1702MX:txtPg1Pt2I17",
    "frm1702MX:txtPg1Pt2I18",
    "frm1702MX:txtPg1Pt2I19",
    "frm1702MX:txtPg1Pt2I20TotalPenalties",
    "frm1702MX:txtPg1Pt2I21TotalAmount",
    "frm1702MX:rdoPg1Pt2I21Refund",
    "frm1702MX:rdoPg1Pt2I21IssueTCC",
    "frm1702MX:rdoPg1Pt2I21CarriedOver",
    "frm1702MX:txtPg1Pt2AuthorizedRepresentative",
    "frm1702MX:txtPg1Pt2Treasurer",
    "frm1702MX:txtPg1P2I23NumOfAttachments",
    "frm1702MX:txtPg1Pt2TitleofSignatory",
    "frm1702MX:txtPg1Pt2TINofSignatory",
    "frm1702MX:txtPg1Pt2TitleofSignatory2",
    "frm1702MX:txtPg1Pt2TINofSignatory2",
    "frm1702MX:txtPg1Pt1I26CashC1",
    "frm1702MX:txtPg1Pt1I26CashC2",
    "frm1702MX:txtPg1Pt1I26CashC3",
    "frm1702MX:txtPg1Pt1I27CheckC1",
    "frm1702MX:txtPg1Pt1I27CheckC2",
    "frm1702MX:txtPg1Pt1I27CheckC3",
    "frm1702MX:txtPg1Pt1I28TaxDebitC2",
    "frm1702MX:txtPg1Pt1I28TaxDebitC3",
    "frm1702MX:txtPg1Pt1I29Others",
    "frm1702MX:txtPg1Pt1I29OthersC1",
    "frm1702MX:txtPg1Pt1I29OthersC2",
    "frm1702MX:txtPg1Pt1I29OthersC3",
    "frm1702MX:txtPg2TIN1",
    "frm1702MX:txtPg2TIN2",
    "frm1702MX:txtPg2TIN3",
    "frm1702MX:txtPg2TIN4",
    "frm1702MX:txtPg2TINMASK",
    "frm1702MX:txtPg2RegisteredName",
    "frm1702MX:InstAPg2Part4",
    "frm1702MX:InstBPg2Part4",
    "frm1702MX:txtPg2Pt4I31CA",
    "frm1702MX:txtPg2Pt4I31CB",
    "frm1702MX:txtPg2Pt4I31CC",
    "frm1702MX:txtPg2Pt4I32CA",
    "frm1702MX:txtPg2Pt4I32CB",
    "frm1702MX:txtPg2Pt4I32CC",
    "frm1702MX:txtPg2Pt4I33CA",
    "frm1702MX:txtPg2Pt4I33CB",
    "frm1702MX:txtPg2Pt4I33CC",
    "frm1702MX:txtPg2Pt4I34SpecialTaxRate",
    "frm1702MX:txtPg2Pt4I35CA",
    "frm1702MX:txtPg2Pt4I35CB",
    "frm1702MX:txtPg2Pt4I35CC",
    "frm1702MX:txtPg2Pt4I36CA",
    "frm1702MX:txtPg2Pt4I36CB",
    "frm1702MX:txtPg2Pt4I36CC",
    "frm1702MX:txtPg2Sc2It14B",
    "frm1702MX:txtPg2Sc2It14C",
    "frm1702MX:txtPg2Sc3It30",
    "frm1702MX:txtPg2Sc3It31",
    "frm1702MX:txtPg3TIN1",
    "frm1702MX:txtPg3TIN2",
    "frm1702MX:txtPg3TIN3",
    "frm1702MX:txtPg3TIN4",
    "frm1702MX:txtPg3TINMASK",
    "frm1702MX:txtPg3RegisteredName",
    "frm1702MX:drpPg3Sc1I11CB",
    "frm1702MX:txtPg3Sc5It17d",
    "frm1702MX:txtPg3Sc5It17e",
    "frm1702MX:txtPg3Sc5It17f",
    "frm1702MX:txtPg3Sc5It17g",
    "frm1702MX:txtPg3Sc5It17h",
    "frm1702MX:txtPg3Sc5It17i",
    "frm1702MX:txtPg6Sc6I1description",
    "frm1702MX:txtPg6Sc6I1legal",
    "frm1702MX:txtPg6Sc6I2description",
    "frm1702MX:txtPg6Sc6I2legal",
    "frm1702MX:txtPg6Sc6I3description",
    "frm1702MX:txtPg6Sc6I3legal",
    "frm1702MX:txtPg3Sc6I4description",
    "frm1702MX:txtPg3Sc6I4legal",
    "frm1702MX:txtPg4TIN1",
    "frm1702MX:txtPg4TIN2",
    "frm1702MX:txtPg4TIN3",
    "frm1702MX:txtPg4TIN4",
    "frm1702MX:txtPg4TINMASK",
    "frm1702MX:txtPg4RegisteredName",
    "frm1702MX:txtPg3IShed7_4Year",
    "frm1702MX:txtPg3IShed7_4A",
    "frm1702MX:txtPg3IShed7_4B",
    "frm1702MX:txtPg3IShed7_4C",
    "frm1702MX:txtPg3IShed7_4D",
    "frm1702MX:txtPg3IShed7_4E",
    "frm1702MX:txtPg3IShed7_5Year",
    "frm1702MX:txtPg3IShed7_5A",
    "frm1702MX:txtPg3IShed7_5B",
    "frm1702MX:txtPg3IShed7_5C",
    "frm1702MX:txtPg3IShed7_5D",
    "frm1702MX:txtPg3IShed7_5E",
    "frm1702MX:txtPg3IShed7_6Year",
    "frm1702MX:txtPg3IShed7_6A",
    "frm1702MX:txtPg3IShed7_6B",
    "frm1702MX:txtPg3IShed7_6C",
    "frm1702MX:txtPg3IShed7_6D",
    "frm1702MX:txtPg3IShed7_6E",
    "frm1702MX:txtPg3IShed7_7Year",
    "frm1702MX:txtPg3IShed7_7A",
    "frm1702MX:txtPg3IShed7_7B",
    "frm1702MX:txtPg3IShed7_7C",
    "frm1702MX:txtPg3IShed7_7D",
    "frm1702MX:txtPg3IShed7_7E",
    "frm1702MX:txtPg3IShed78D",
    "frm1702MX:txtPg3IShed8_4Year",
    "frm1702MX:txtPg4IShed8_4A",
    "frm1702MX:txtPg4IShed8_4B",
    "frm1702MX:txtPg4IShed8_4C",
    "frm1702MX:txtPg4IShed8_4D",
    "frm1702MX:txtPg4IShed8_4E",
    "frm1702MX:txtPg3IShed8_5Year",
    "frm1702MX:txtPg4IShed8_5A",
    "frm1702MX:txtPg4IShed8_5B",
    "frm1702MX:txtPg4IShed8_5C",
    "frm1702MX:txtPg4IShed8_5D",
    "frm1702MX:txtPg4IShed8_5E",
    "frm1702MX:txtPg3IShed8_6Year",
    "frm1702MX:txtPg4IShed8_6A",
    "frm1702MX:txtPg4IShed8_6B",
    "frm1702MX:txtPg3IShed8_6C",
    "frm1702MX:txtPg4IShed8_6D",
    "frm1702MX:txtPg4IShed8_6E",
    "frm1702MX:txtPg4IShed8_7Year",
    "frm1702MX:txtPg4IShed8_7A",
    "frm1702MX:txtPg4IShed8_7B",
    "frm1702MX:txtPg4IShed8_7C",
    "frm1702MX:txtPg4IShed8_7D",
    "frm1702MX:txtPg4IShed8_7E",
    "frm1702MX:txtPg3IShed88D",
    "frm1702MX:txtPg7Sc9I1",
    "frm1702MX:txtPg7Sc9I2",
    "frm1702MX:txtPg7Sc9I3",
    "frm1702MX:txtPg4Sc10Itm2",
    "frm1702MX:txtPg4Sc10Itm3",
    "frm1702MX:txtPg4Sc10Itm5",
    "frm1702MX:txtPg4Sc10Itm6",
    "frm1702MX:txtPg4Sc10Itm7",
    "frm1702MX:txtPg4Sc10Itm8",
    "attachmentCurrent",
    "attachmentTotal",
    "frm1702MX:txtPg1AttMTIN1",
    "frm1702MX:txtPg1AttMTIN2",
    "frm1702MX:txtPg1AttMTIN3",
    "frm1702MX:txtPg1AttMTIN4",
    "frm1702MX:txtPg1AttMTINMASK",
    "frm1702MX:rdoPg1AttMExempt",
    "frm1702MX:rdoPg1AttMSpecialRate",
    "frm1702MX:txtPg1AttMPt5ScAIt5",
    "frm1702MX:txtPg1AttMPt5ScAIt6",
    "frm1702MX:txtPg2AttMTIN1",
    "frm1702MX:txtPg2AttMTIN2",
    "frm1702MX:txtPg2AttMTIN3",
    "frm1702MX:txtPg2AttMTIN4",
    "frm1702MX:txtPg2AttMTINMASK",
    "frm1702MX:txtPg2AttMDesc20",
    "frm1702MX:txtPg2AttMDesc21",
    "frm1702MX:txtPg2AttMDesc22",
    "frm1702MX:txtPg2AttMDesc23",
    "frm1702MX:txtPg2AttMDesc24",
    "frm1702MX:txtPg2AttMScDI17iother",
    "frm1702MX:txtPg2AttMScF1I4year",
    "frm1702MX:txtCurrentPage",
    "frm1702MX:txtMaxPage",
    "frm1702MX:txtCtrmodalPg7Sc8",
    "frm1702MX:txtCtrmodalPg3Sc5I17i",
    "frm1702MX:txtCtrmodalPg6Sc6",
    "frm1702MX:txtCtrmodalPg7Sc10I3",
    "frm1702MX:txtCtrmodalPg7Sc10I6",
    "frm1702MX:txtCtrmodalPg7Sc10I8",
    "frm1702MX:totalEXAttach",
    "frm1702MX:totalSPAttach",
    "txtFinalFlag",
    "txtEnroll",
    "ebirOnlineConfirmUsername",
    "ebirOnlineUsername",
    "ebirOnlineSecret",
    "driveSelectTPExport",
];

impl Form1702MXDraft {
    pub fn to_bir_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = EXPECTED_SOURCE_KEYS
            .iter()
            .map(|key| ((*key).to_string(), String::new()))
            .collect::<BTreeMap<_, _>>();
        for (key, value) in &self.preserved_xml_fields {
            if fields.contains_key(key) {
                fields.insert(key.clone(), value.clone());
            }
        }
        let (tin1, tin2, tin3, branch) = split_tin(&self.tin);

        insert_bool(
            &mut fields,
            "frm1702MX:rdoPg1I1Calendar",
            matches!(self.filing_basis, Form1702MXFilingBasis::Calendar),
        );
        insert_bool(
            &mut fields,
            "frm1702MX:rdoPg1I1Fiscal",
            matches!(self.filing_basis, Form1702MXFilingBasis::Fiscal),
        );
        insert(
            &mut fields,
            "frm1702MX:ddlPg1I2Date",
            format!("{:02}", self.month),
        );
        insert(
            &mut fields,
            "frm1702MX:txtPg1I2YearEnd",
            format!("{:02}", self.taxable_year % 100),
        );
        insert_bool(&mut fields, "frm1702MX:rdoPg1I3AmendedYes", self.is_amended);
        insert_bool(&mut fields, "frm1702MX:rdoPg1I3AmendedNo", !self.is_amended);
        insert_bool(
            &mut fields,
            "frm1702MX:rdoPg1I4ShortPeriodYes",
            self.is_short_period,
        );
        insert_bool(
            &mut fields,
            "frm1702MX:rdoPg1I4ShortPeriodNo",
            !self.is_short_period,
        );
        insert_bool(
            &mut fields,
            "frm1702MX:chkPg1I5ATCR1",
            self.atc.mcit_selected,
        );
        insert(
            &mut fields,
            "frm1702MX:drpPg1I5ATCR2",
            self.atc.other_code.clone(),
        );
        insert_bool(
            &mut fields,
            "frm1702MX:chkPg1I5ATCR2",
            self.atc.other_selected,
        );

        for (key, value) in [
            ("frm1702MX:txtPg1Pt1I6TINC1", tin1.as_str()),
            ("frm1702MX:txtPg1Pt1I6TINC2", tin2.as_str()),
            ("frm1702MX:txtPg1Pt1I6TINC3", tin3.as_str()),
            ("frm1702MX:txtPg1Pt1I6TINC4", branch.as_str()),
            ("frm1702MX:txtPg1TINMASK", branch.as_str()),
        ] {
            insert(&mut fields, key, value);
        }
        insert(
            &mut fields,
            "frm1702MX:txtPg1Pt1I7RDO",
            self.rdo_code.clone(),
        );
        insert(
            &mut fields,
            "frm1702MX:drpPg1Pt1I7RDO",
            self.rdo_code.clone(),
        );
        for (index, value) in self.registered_name_lines.iter().enumerate() {
            insert(
                &mut fields,
                &format!(
                    "frm1702MX:txtPg1Pt1I9RegisteredName{}",
                    if index == 0 {
                        String::new()
                    } else {
                        (index + 1).to_string()
                    }
                ),
                value.clone(),
            );
        }
        for (index, value) in self.registered_address_lines.iter().enumerate() {
            insert(
                &mut fields,
                &format!(
                    "frm1702MX:txtPg1Pt1I10RegisteredAddress{}",
                    if index == 0 {
                        String::new()
                    } else {
                        (index + 1).to_string()
                    }
                ),
                value.clone(),
            );
        }
        insert(&mut fields, "frm1702MX:txtZIP", self.zip_code.clone());
        insert(
            &mut fields,
            "frm1702MX:txtPg1Pt1I8",
            self.incorporation_date.clone(),
        );
        insert(
            &mut fields,
            "frm1702MX:txtPg1Pt1I11ContactNumber",
            self.contact_number.clone(),
        );
        insert(
            &mut fields,
            "frm1702MX:txtPg1Pt1I12Email",
            self.email.clone(),
        );
        insert_bool(
            &mut fields,
            "frm1702MX:rdoPg1Pt1I13MethodOfDeducItemized",
            matches!(self.deduction_method, Form1702MXDeductionMethod::Itemized),
        );
        insert_bool(
            &mut fields,
            "frm1702MX:rdoPg1Pt1I13MethodOfDeducOptional",
            matches!(
                self.deduction_method,
                Form1702MXDeductionMethod::OptionalStandard
            ),
        );

        insert_money(
            &mut fields,
            "frm1702MX:txtPg1Pt2I17",
            &self.part_ii.item_17_surcharge,
        );
        insert_money(
            &mut fields,
            "frm1702MX:txtPg1Pt2I18",
            &self.part_ii.item_18_interest,
        );
        insert_money(
            &mut fields,
            "frm1702MX:txtPg1Pt2I19",
            &self.part_ii.item_19_compromise,
        );
        insert_money(
            &mut fields,
            "frm1702MX:txtPg1Pt2I20TotalPenalties",
            &self.part_ii.item_20_total_penalties,
        );
        insert_money(
            &mut fields,
            "frm1702MX:txtPg1Pt2I21TotalAmount",
            &self.part_ii.item_21_total_amount_payable_or_overpayment,
        );
        insert_bool(
            &mut fields,
            "frm1702MX:rdoPg1Pt2I21Refund",
            matches!(
                self.part_ii.overpayment_disposition,
                Some(Form1702MXOverpaymentDisposition::Refund)
            ),
        );
        insert_bool(
            &mut fields,
            "frm1702MX:rdoPg1Pt2I21IssueTCC",
            matches!(
                self.part_ii.overpayment_disposition,
                Some(Form1702MXOverpaymentDisposition::TaxCreditCertificate)
            ),
        );
        insert_bool(
            &mut fields,
            "frm1702MX:rdoPg1Pt2I21CarriedOver",
            matches!(
                self.part_ii.overpayment_disposition,
                Some(Form1702MXOverpaymentDisposition::CarryOver)
            ),
        );
        insert(
            &mut fields,
            "frm1702MX:txtPg1Pt2AuthorizedRepresentative",
            self.authorized_representative.clone(),
        );
        insert(
            &mut fields,
            "frm1702MX:txtPg1Pt2Treasurer",
            self.treasurer.clone(),
        );
        insert(
            &mut fields,
            "frm1702MX:txtPg1P2I23NumOfAttachments",
            self.number_of_attachments.clone(),
        );
        insert(
            &mut fields,
            "frm1702MX:txtPg1Pt2TitleofSignatory",
            self.president_title.clone(),
        );
        insert(
            &mut fields,
            "frm1702MX:txtPg1Pt2TINofSignatory",
            self.president_tin.clone(),
        );
        insert(
            &mut fields,
            "frm1702MX:txtPg1Pt2TitleofSignatory2",
            self.treasurer_title.clone(),
        );
        insert(
            &mut fields,
            "frm1702MX:txtPg1Pt2TINofSignatory2",
            self.treasurer_tin.clone(),
        );
        insert_payment_details(&mut fields, &self.payment_details);

        for page in 2..=4 {
            for (segment, value) in [tin1.as_str(), tin2.as_str(), tin3.as_str(), branch.as_str()]
                .into_iter()
                .enumerate()
            {
                insert(
                    &mut fields,
                    &format!("frm1702MX:txtPg{page}TIN{}", segment + 1),
                    value,
                );
            }
            insert(
                &mut fields,
                &format!("frm1702MX:txtPg{page}TINMASK"),
                branch.clone(),
            );
            insert(
                &mut fields,
                &format!("frm1702MX:txtPg{page}RegisteredName"),
                self.taxpayer_name.clone(),
            );
        }
        // Part IV Schedule 1's A/B instructions are one radio pair: A is
        // checked unless B is (fields.json default A "C", B "F"), as the
        // official page writes them (`form_1702mx_official`). The page sets
        // only `instruction_multiple_activities`, so the single-activity
        // flag alone would leave both unchecked.
        insert_bool(
            &mut fields,
            "frm1702MX:InstAPg2Part4",
            !self.relief_basis.instruction_multiple_activities,
        );
        insert_bool(
            &mut fields,
            "frm1702MX:InstBPg2Part4",
            self.relief_basis.instruction_multiple_activities,
        );
        insert_percent(
            &mut fields,
            "frm1702MX:txtPg2Pt4I34SpecialTaxRate",
            &self.relief_basis.special_tax_rate,
        );
        for (source_item, model_index) in [(31, 11_usize), (32, 12), (33, 13)] {
            let amounts = &self.schedule_3.items_20_to_33[model_index];
            for (suffix, value) in [
                ("A", &amounts.exempt),
                ("B", &amounts.special),
                ("C", &amounts.regular),
            ] {
                insert_money(
                    &mut fields,
                    &format!("frm1702MX:txtPg2Pt4I{source_item}C{suffix}"),
                    value,
                );
            }
        }
        insert_percent(
            &mut fields,
            "frm1702MX:txtPg2Sc2It14B",
            &self.schedule_2.item_14_special_rate,
        );
        insert_percent(
            &mut fields,
            "frm1702MX:txtPg2Sc2It14C",
            &self.schedule_2.item_14_regular_rate,
        );
        insert(
            &mut fields,
            "frm1702MX:txtPg2Sc3It30",
            self.schedule_3.item_30_description.clone(),
        );
        insert(
            &mut fields,
            "frm1702MX:txtPg2Sc3It31",
            self.schedule_3.item_31_description.clone(),
        );

        for (index, description) in self
            .schedule_5
            .other_descriptions_17d_to_17i
            .iter()
            .enumerate()
        {
            insert(
                &mut fields,
                &format!("frm1702MX:txtPg3Sc5It17{}", (b'd' + index as u8) as char),
                description.clone(),
            );
        }
        for (index, row) in self.schedule_6.rows.iter().enumerate() {
            let page = if index < 3 { 6 } else { 3 };
            insert(
                &mut fields,
                &format!("frm1702MX:txtPg{page}Sc6I{}description", index + 1),
                row.description.clone(),
            );
            insert(
                &mut fields,
                &format!("frm1702MX:txtPg{page}Sc6I{}legal", index + 1),
                row.legal_basis.clone(),
            );
        }
        insert_nolco_table(&mut fields, 7, &self.schedule_7_1);
        insert_nolco_table(&mut fields, 8, &self.schedule_8_1);
        for (index, row) in self.schedule_9.rows.iter().enumerate() {
            insert(
                &mut fields,
                &format!("frm1702MX:txtPg7Sc9I{}", index + 1),
                row.year.clone(),
            );
        }
        for index in [1_usize, 2, 4, 5, 6, 7] {
            insert(
                &mut fields,
                &format!("frm1702MX:txtPg4Sc10Itm{}", index + 1),
                self.schedule_10.descriptions[index].clone(),
            );
        }
        insert_attachment_fields(&mut fields, self, &tin1, &tin2, &tin3, &branch);
        insert_transport_fields(&mut fields, self);
        fields
    }

    pub fn to_bir_xml_payload(&self) -> String {
        crate::bir_xml::generate_bir_xml(&self.to_bir_field_map())
    }

    pub fn try_to_bir_xml_payload(&self) -> Result<String, Vec<(String, String)>> {
        let errors = self.validate();
        if errors.is_empty() {
            Ok(self.to_bir_xml_payload())
        } else {
            Err(errors)
        }
    }

    pub fn from_bir_xml_payload(xml: &str) -> Result<Self, Vec<(String, String)>> {
        let fields = crate::bir_xml::parse_bir_xml_checked(xml).map_err(|error| {
            vec![(
                "xml".to_string(),
                format!("Invalid BIR pseudo-XML: {error}"),
            )]
        })?;
        Self::from_bir_field_map(&fields)
    }

    pub fn from_bir_field_map(
        fields: &BTreeMap<String, String>,
    ) -> Result<Self, Vec<(String, String)>> {
        let mut errors = Vec::new();
        let expected = EXPECTED_SOURCE_KEYS
            .iter()
            .map(|key| (*key).to_string())
            .collect::<BTreeSet<_>>();
        let actual = fields.keys().cloned().collect::<BTreeSet<_>>();
        for missing in expected.difference(&actual) {
            errors.push((
                missing.clone(),
                "Required 1702MX source field is missing".to_string(),
            ));
        }
        for unexpected in actual.difference(&expected) {
            errors.push((
                unexpected.clone(),
                "Field is not part of the reviewed 1702MXv2018C contract".to_string(),
            ));
        }
        for (key, expected_value) in [
            ("frm1702MX:txtCurrentPage", "1"),
            ("frm1702MX:txtMaxPage", "4"),
            ("driveSelectTPExport", "0"),
            ("txtEnroll", "Y"),
            ("ebirOnlineConfirmUsername", ""),
            ("ebirOnlineUsername", ""),
            ("ebirOnlineSecret", ""),
        ] {
            require_exact(fields, key, expected_value, &mut errors);
        }
        if !matches!(field(fields, "txtFinalFlag"), "0" | "1") {
            errors.push((
                "txtFinalFlag".to_string(),
                "Expected observed editable/encrypted flag value 0 or 1".to_string(),
            ));
        }

        let filing_basis = parse_exclusive_pair(
            fields,
            "frm1702MX:rdoPg1I1Calendar",
            "frm1702MX:rdoPg1I1Fiscal",
            "filing_basis",
            &mut errors,
        )
        .map(|calendar| {
            if calendar {
                Form1702MXFilingBasis::Calendar
            } else {
                Form1702MXFilingBasis::Fiscal
            }
        })
        .unwrap_or_default();
        let is_amended = parse_exclusive_pair(
            fields,
            "frm1702MX:rdoPg1I3AmendedYes",
            "frm1702MX:rdoPg1I3AmendedNo",
            "is_amended",
            &mut errors,
        )
        .unwrap_or(false);
        let is_short_period = parse_exclusive_pair(
            fields,
            "frm1702MX:rdoPg1I4ShortPeriodYes",
            "frm1702MX:rdoPg1I4ShortPeriodNo",
            "is_short_period",
            &mut errors,
        )
        .unwrap_or(false);
        let itemized = parse_bool(
            fields,
            "frm1702MX:rdoPg1Pt1I13MethodOfDeducItemized",
            &mut errors,
        );
        let optional = parse_bool(
            fields,
            "frm1702MX:rdoPg1Pt1I13MethodOfDeducOptional",
            &mut errors,
        );
        let deduction_method = match (itemized, optional) {
            (Some(true), Some(false)) => Form1702MXDeductionMethod::Itemized,
            (Some(false), Some(true)) => Form1702MXDeductionMethod::OptionalStandard,
            (Some(false), Some(false)) => Form1702MXDeductionMethod::Unresolved,
            (Some(true), Some(true)) => {
                errors.push((
                    "deduction_method".to_string(),
                    "Item 13 choices cannot both be selected".to_string(),
                ));
                Form1702MXDeductionMethod::Unresolved
            }
            _ => Form1702MXDeductionMethod::Unresolved,
        };

        let tin_segments = [
            field(fields, "frm1702MX:txtPg1Pt1I6TINC1"),
            field(fields, "frm1702MX:txtPg1Pt1I6TINC2"),
            field(fields, "frm1702MX:txtPg1Pt1I6TINC3"),
            field(fields, "frm1702MX:txtPg1Pt1I6TINC4"),
        ];
        for page in 2..=4 {
            for (index, source) in tin_segments.iter().enumerate() {
                verify_equal(
                    fields,
                    &format!("frm1702MX:txtPg{page}TIN{}", index + 1),
                    source,
                    &mut errors,
                );
            }
        }
        verify_equal(
            fields,
            "frm1702MX:drpPg1Pt1I7RDO",
            field(fields, "frm1702MX:txtPg1Pt1I7RDO"),
            &mut errors,
        );
        let registered_name_lines = std::array::from_fn(|index| {
            field(
                fields,
                &format!(
                    "frm1702MX:txtPg1Pt1I9RegisteredName{}",
                    if index == 0 {
                        String::new()
                    } else {
                        (index + 1).to_string()
                    }
                ),
            )
            .to_string()
        });
        let registered_address_lines = std::array::from_fn(|index| {
            field(
                fields,
                &format!(
                    "frm1702MX:txtPg1Pt1I10RegisteredAddress{}",
                    if index == 0 {
                        String::new()
                    } else {
                        (index + 1).to_string()
                    }
                ),
            )
            .to_string()
        });
        let taxpayer_name = field(fields, "frm1702MX:txtPg2RegisteredName").to_string();
        for page in 3..=4 {
            verify_equal(
                fields,
                &format!("frm1702MX:txtPg{page}RegisteredName"),
                &taxpayer_name,
                &mut errors,
            );
        }

        let overpayment_disposition = parse_overpayment_disposition(fields, &mut errors);
        let schedule_2 = super::form_1702mx::Form1702MXSchedule2 {
            item_14_special_rate: parse_percent(fields, "frm1702MX:txtPg2Sc2It14B", &mut errors),
            item_14_regular_rate: parse_percent(fields, "frm1702MX:txtPg2Sc2It14C", &mut errors),
            ..super::form_1702mx::Form1702MXSchedule2::default()
        };
        let mut schedule_3 = super::form_1702mx::Form1702MXSchedule3 {
            item_30_description: field(fields, "frm1702MX:txtPg2Sc3It30").to_string(),
            item_31_description: field(fields, "frm1702MX:txtPg2Sc3It31").to_string(),
            ..super::form_1702mx::Form1702MXSchedule3::default()
        };
        for (source_item, model_index) in [(31, 11_usize), (32, 12), (33, 13)] {
            let row = &mut schedule_3.items_20_to_33[model_index];
            row.exempt = parse_money(
                fields,
                &format!("frm1702MX:txtPg2Pt4I{source_item}CA"),
                &mut errors,
            );
            row.special = parse_money(
                fields,
                &format!("frm1702MX:txtPg2Pt4I{source_item}CB"),
                &mut errors,
            );
            row.regular = parse_money(
                fields,
                &format!("frm1702MX:txtPg2Pt4I{source_item}CC"),
                &mut errors,
            );
        }
        let mut schedule_5 = super::form_1702mx::Form1702MXSchedule5::default();
        for (index, value) in schedule_5
            .other_descriptions_17d_to_17i
            .iter_mut()
            .enumerate()
        {
            *value = field(
                fields,
                &format!("frm1702MX:txtPg3Sc5It17{}", (b'd' + index as u8) as char),
            )
            .to_string();
        }
        let mut schedule_6 = super::form_1702mx::Form1702MXSchedule6::default();
        for (index, row) in schedule_6.rows.iter_mut().enumerate() {
            let page = if index < 3 { 6 } else { 3 };
            row.description = field(
                fields,
                &format!("frm1702MX:txtPg{page}Sc6I{}description", index + 1),
            )
            .to_string();
            row.legal_basis = field(
                fields,
                &format!("frm1702MX:txtPg{page}Sc6I{}legal", index + 1),
            )
            .to_string();
        }
        let schedule_7_1 = parse_nolco_table(fields, 7, &mut errors);
        let schedule_8_1 = parse_nolco_table(fields, 8, &mut errors);
        let mut schedule_9 = super::form_1702mx::Form1702MXSchedule9::default();
        for (index, row) in schedule_9.rows.iter_mut().enumerate() {
            row.year = field(fields, &format!("frm1702MX:txtPg7Sc9I{}", index + 1)).to_string();
        }
        let mut schedule_10 = super::form_1702mx::Form1702MXSchedule10::default();
        for index in [1_usize, 2, 4, 5, 6, 7] {
            schedule_10.descriptions[index] =
                field(fields, &format!("frm1702MX:txtPg4Sc10Itm{}", index + 1)).to_string();
        }

        let mut draft = Self {
            penalty: crate::forms::penalty_items::PenaltyState::imported(),
            id: None,
            tin: tin_segments.concat(),
            taxable_year: parse_taxable_year(fields, "frm1702MX:txtPg1I2YearEnd", &mut errors),
            month: parse_u8(fields, "frm1702MX:ddlPg1I2Date", &mut errors),
            filing_basis,
            is_amended,
            is_short_period,
            atc: Form1702MXAtcSelection {
                mcit_selected: parse_bool(fields, "frm1702MX:chkPg1I5ATCR1", &mut errors)
                    .unwrap_or(false),
                other_selected: parse_bool(fields, "frm1702MX:chkPg1I5ATCR2", &mut errors)
                    .unwrap_or(false),
                other_code: field(fields, "frm1702MX:drpPg1I5ATCR2").to_string(),
            },
            rdo_code: field(fields, "frm1702MX:txtPg1Pt1I7RDO").to_string(),
            taxpayer_name,
            registered_name_lines,
            registered_address: registered_address_lines
                .iter()
                .filter(|value| !value.trim().is_empty())
                .cloned()
                .collect::<Vec<_>>()
                .join(" "),
            registered_address_lines,
            zip_code: field(fields, "frm1702MX:txtZIP").to_string(),
            incorporation_date: field(fields, "frm1702MX:txtPg1Pt1I8").to_string(),
            contact_number: field(fields, "frm1702MX:txtPg1Pt1I11ContactNumber").to_string(),
            email: field(fields, "frm1702MX:txtPg1Pt1I12Email").to_string(),
            deduction_method,
            part_ii: Form1702MXPartII {
                item_14_total_tax_due_or_overpayment: WholePeso::default(),
                item_15_total_tax_credits: WholePeso::default(),
                item_16_net_tax_payable_or_overpayment: WholePeso::default(),
                item_17_surcharge: parse_money(fields, "frm1702MX:txtPg1Pt2I17", &mut errors),
                item_18_interest: parse_money(fields, "frm1702MX:txtPg1Pt2I18", &mut errors),
                item_19_compromise: parse_money(fields, "frm1702MX:txtPg1Pt2I19", &mut errors),
                item_20_total_penalties: parse_money(
                    fields,
                    "frm1702MX:txtPg1Pt2I20TotalPenalties",
                    &mut errors,
                ),
                item_21_total_amount_payable_or_overpayment: parse_money(
                    fields,
                    "frm1702MX:txtPg1Pt2I21TotalAmount",
                    &mut errors,
                ),
                overpayment_disposition,
            },
            relief_basis: super::form_1702mx::Form1702MXReliefBasis {
                instruction_single_activity: parse_bool(
                    fields,
                    "frm1702MX:InstAPg2Part4",
                    &mut errors,
                )
                .unwrap_or(false),
                instruction_multiple_activities: parse_bool(
                    fields,
                    "frm1702MX:InstBPg2Part4",
                    &mut errors,
                )
                .unwrap_or(false),
                special_tax_rate: parse_percent(
                    fields,
                    "frm1702MX:txtPg2Pt4I34SpecialTaxRate",
                    &mut errors,
                ),
            },
            schedule_2,
            schedule_3,
            schedule_4: super::form_1702mx::Form1702MXSchedule4::default(),
            schedule_5,
            schedule_6,
            regular_nolco: super::form_1702mx::Form1702MXNolcoComputation::default(),
            schedule_7_1,
            special_nolco: super::form_1702mx::Form1702MXNolcoComputation::default(),
            schedule_8_1,
            schedule_9,
            schedule_10,
            mandatory_attachment: parse_attachment(fields, &mut errors),
            authorized_representative: field(fields, "frm1702MX:txtPg1Pt2AuthorizedRepresentative")
                .to_string(),
            treasurer: field(fields, "frm1702MX:txtPg1Pt2Treasurer").to_string(),
            number_of_attachments: field(fields, "frm1702MX:txtPg1P2I23NumOfAttachments")
                .to_string(),
            president_title: field(fields, "frm1702MX:txtPg1Pt2TitleofSignatory").to_string(),
            president_tin: field(fields, "frm1702MX:txtPg1Pt2TINofSignatory").to_string(),
            treasurer_title: field(fields, "frm1702MX:txtPg1Pt2TitleofSignatory2").to_string(),
            treasurer_tin: field(fields, "frm1702MX:txtPg1Pt2TINofSignatory2").to_string(),
            payment_details: parse_payment_details(fields),
            xml_final_flag: field(fields, "txtFinalFlag").to_string(),
            preserved_xml_fields: fields.clone(),
            calculation_issues: Vec::new(),
            relief_details: Default::default(),
            last_error: None,
            lifecycle: super::queueable::SubmissionLifecycle::default(),
        };
        // An imported return carries the penalties it was prepared with:
        // keep each one as an override so the source amounts round-trip.
        Form1702MXDraft::penalty_declaration().adopt_imported(&mut draft);
        if errors.is_empty() {
            Ok(draft)
        } else {
            Err(errors)
        }
    }
}

fn insert_payment_details(
    fields: &mut BTreeMap<String, String>,
    rows: &[Form1702MXPaymentDetail; 4],
) {
    for (key, value) in [
        ("frm1702MX:txtPg1Pt1I26CashC1", rows[0].drawee.as_str()),
        ("frm1702MX:txtPg1Pt1I26CashC2", rows[0].number.as_str()),
        (
            "frm1702MX:txtPg1Pt1I26CashC3",
            rows[0].date_or_amount.as_str(),
        ),
        ("frm1702MX:txtPg1Pt1I27CheckC1", rows[1].drawee.as_str()),
        ("frm1702MX:txtPg1Pt1I27CheckC2", rows[1].number.as_str()),
        (
            "frm1702MX:txtPg1Pt1I27CheckC3",
            rows[1].date_or_amount.as_str(),
        ),
        ("frm1702MX:txtPg1Pt1I28TaxDebitC2", rows[2].number.as_str()),
        (
            "frm1702MX:txtPg1Pt1I28TaxDebitC3",
            rows[2].date_or_amount.as_str(),
        ),
        ("frm1702MX:txtPg1Pt1I29Others", rows[3].particulars.as_str()),
        ("frm1702MX:txtPg1Pt1I29OthersC1", rows[3].drawee.as_str()),
        ("frm1702MX:txtPg1Pt1I29OthersC2", rows[3].number.as_str()),
        (
            "frm1702MX:txtPg1Pt1I29OthersC3",
            rows[3].date_or_amount.as_str(),
        ),
    ] {
        insert(fields, key, value);
    }
}

fn parse_payment_details(fields: &BTreeMap<String, String>) -> [Form1702MXPaymentDetail; 4] {
    [
        Form1702MXPaymentDetail {
            particulars: "Cash/Bank Debit Memo".to_string(),
            drawee: field(fields, "frm1702MX:txtPg1Pt1I26CashC1").to_string(),
            number: field(fields, "frm1702MX:txtPg1Pt1I26CashC2").to_string(),
            date_or_amount: field(fields, "frm1702MX:txtPg1Pt1I26CashC3").to_string(),
        },
        Form1702MXPaymentDetail {
            particulars: "Check".to_string(),
            drawee: field(fields, "frm1702MX:txtPg1Pt1I27CheckC1").to_string(),
            number: field(fields, "frm1702MX:txtPg1Pt1I27CheckC2").to_string(),
            date_or_amount: field(fields, "frm1702MX:txtPg1Pt1I27CheckC3").to_string(),
        },
        Form1702MXPaymentDetail {
            particulars: "Tax Debit Memo".to_string(),
            drawee: String::new(),
            number: field(fields, "frm1702MX:txtPg1Pt1I28TaxDebitC2").to_string(),
            date_or_amount: field(fields, "frm1702MX:txtPg1Pt1I28TaxDebitC3").to_string(),
        },
        Form1702MXPaymentDetail {
            particulars: field(fields, "frm1702MX:txtPg1Pt1I29Others").to_string(),
            drawee: field(fields, "frm1702MX:txtPg1Pt1I29OthersC1").to_string(),
            number: field(fields, "frm1702MX:txtPg1Pt1I29OthersC2").to_string(),
            date_or_amount: field(fields, "frm1702MX:txtPg1Pt1I29OthersC3").to_string(),
        },
    ]
}

fn insert_nolco_table(
    fields: &mut BTreeMap<String, String>,
    schedule: u8,
    table: &super::form_1702mx::Form1702MXNolcoTable,
) {
    for (index, row) in table.rows.iter().enumerate() {
        let item = index + 4;
        let year_page = if schedule == 7 || index < 3 { 3 } else { 4 };
        insert(
            fields,
            &format!("frm1702MX:txtPg{year_page}IShed{schedule}_{item}Year"),
            row.year_incurred.clone(),
        );
        let amount_page = if schedule == 7 { 3 } else { 4 };
        for (suffix, value) in [
            ("A", &row.amount),
            ("B", &row.applied_previous_years),
            ("C", &row.expired),
            ("D", &row.applied_current_year),
            ("E", &row.unapplied),
        ] {
            let page = if schedule == 8 && index == 2 && suffix == "C" {
                3
            } else {
                amount_page
            };
            insert_money(
                fields,
                &format!("frm1702MX:txtPg{page}IShed{schedule}_{item}{suffix}"),
                value,
            );
        }
    }
    insert_money(
        fields,
        &format!("frm1702MX:txtPg3IShed{schedule}8D"),
        &table.item_8_total_applied_current_year,
    );
}

fn parse_nolco_table(
    fields: &BTreeMap<String, String>,
    schedule: u8,
    errors: &mut Vec<(String, String)>,
) -> super::form_1702mx::Form1702MXNolcoTable {
    let mut table = super::form_1702mx::Form1702MXNolcoTable::default();
    for (index, row) in table.rows.iter_mut().enumerate() {
        let item = index + 4;
        let year_page = if schedule == 7 || index < 3 { 3 } else { 4 };
        row.year_incurred = field(
            fields,
            &format!("frm1702MX:txtPg{year_page}IShed{schedule}_{item}Year"),
        )
        .to_string();
        let amount_page = if schedule == 7 { 3 } else { 4 };
        let parse_cell = |suffix: &str, errors: &mut Vec<(String, String)>| {
            let page = if schedule == 8 && index == 2 && suffix == "C" {
                3
            } else {
                amount_page
            };
            parse_money(
                fields,
                &format!("frm1702MX:txtPg{page}IShed{schedule}_{item}{suffix}"),
                errors,
            )
        };
        row.amount = parse_cell("A", errors);
        row.applied_previous_years = parse_cell("B", errors);
        row.expired = parse_cell("C", errors);
        row.applied_current_year = parse_cell("D", errors);
        row.unapplied = parse_cell("E", errors);
    }
    table.item_8_total_applied_current_year = parse_money(
        fields,
        &format!("frm1702MX:txtPg3IShed{schedule}8D"),
        errors,
    );
    table
}

fn insert_attachment_fields(
    fields: &mut BTreeMap<String, String>,
    draft: &Form1702MXDraft,
    tin1: &str,
    tin2: &str,
    tin3: &str,
    branch: &str,
) {
    let attachment = &draft.mandatory_attachment;
    insert(
        fields,
        "attachmentCurrent",
        if attachment.current_index.is_empty() {
            "0"
        } else {
            &attachment.current_index
        },
    );
    insert(
        fields,
        "attachmentTotal",
        if attachment.total_count.is_empty() {
            "0"
        } else {
            &attachment.total_count
        },
    );
    for page in 1..=2 {
        for (index, value) in [tin1, tin2, tin3, branch].into_iter().enumerate() {
            insert(
                fields,
                &format!("frm1702MX:txtPg{page}AttMTIN{}", index + 1),
                value,
            );
        }
    }
    insert_bool(
        fields,
        "frm1702MX:rdoPg1AttMExempt",
        attachment.exempt_activity,
    );
    insert_bool(
        fields,
        "frm1702MX:rdoPg1AttMSpecialRate",
        attachment.special_rate_activity,
    );
    insert(
        fields,
        "frm1702MX:txtPg1AttMPt5ScAIt5",
        attachment.schedule_a_effectivity_from.clone(),
    );
    insert(
        fields,
        "frm1702MX:txtPg1AttMPt5ScAIt6",
        attachment.schedule_a_effectivity_until.clone(),
    );
    for (index, value) in attachment.descriptions_20_to_24.iter().enumerate() {
        insert(
            fields,
            &format!("frm1702MX:txtPg2AttMDesc{}", index + 20),
            value.clone(),
        );
    }
    insert(
        fields,
        "frm1702MX:txtPg2AttMScDI17iother",
        attachment.schedule_d_other_description.clone(),
    );
    insert(
        fields,
        "frm1702MX:txtPg2AttMScF1I4year",
        attachment.schedule_f_year.clone(),
    );
}

fn parse_attachment(
    fields: &BTreeMap<String, String>,
    errors: &mut Vec<(String, String)>,
) -> Form1702MXMandatoryAttachment {
    Form1702MXMandatoryAttachment {
        current_index: field(fields, "attachmentCurrent").to_string(),
        total_count: field(fields, "attachmentTotal").to_string(),
        exempt_activity: parse_bool(fields, "frm1702MX:rdoPg1AttMExempt", errors).unwrap_or(false),
        special_rate_activity: parse_bool(fields, "frm1702MX:rdoPg1AttMSpecialRate", errors)
            .unwrap_or(false),
        schedule_a_effectivity_from: field(fields, "frm1702MX:txtPg1AttMPt5ScAIt5").to_string(),
        schedule_a_effectivity_until: field(fields, "frm1702MX:txtPg1AttMPt5ScAIt6").to_string(),
        schedule_d_other_description: field(fields, "frm1702MX:txtPg2AttMScDI17iother").to_string(),
        schedule_f_year: field(fields, "frm1702MX:txtPg2AttMScF1I4year").to_string(),
        descriptions_20_to_24: std::array::from_fn(|index| {
            field(fields, &format!("frm1702MX:txtPg2AttMDesc{}", index + 20)).to_string()
        }),
    }
}

fn insert_transport_fields(fields: &mut BTreeMap<String, String>, draft: &Form1702MXDraft) {
    for key in [
        "frm1702MX:txtCtrmodalPg7Sc8",
        "frm1702MX:txtCtrmodalPg3Sc5I17i",
        "frm1702MX:txtCtrmodalPg6Sc6",
        "frm1702MX:txtCtrmodalPg7Sc10I3",
        "frm1702MX:txtCtrmodalPg7Sc10I6",
        "frm1702MX:txtCtrmodalPg7Sc10I8",
        "frm1702MX:totalEXAttach",
        "frm1702MX:totalSPAttach",
    ] {
        if field(fields, key).is_empty() {
            insert(fields, key, "0");
        }
    }
    insert(fields, "frm1702MX:txtCurrentPage", "1");
    insert(fields, "frm1702MX:txtMaxPage", "4");
    insert(fields, "txtFinalFlag", draft.xml_final_flag.clone());
    insert(fields, "txtEnroll", "Y");
    insert(fields, "ebirOnlineConfirmUsername", "");
    insert(fields, "ebirOnlineUsername", "");
    insert(fields, "ebirOnlineSecret", "");
    insert(fields, "driveSelectTPExport", "0");
}

fn parse_overpayment_disposition(
    fields: &BTreeMap<String, String>,
    errors: &mut Vec<(String, String)>,
) -> Option<Form1702MXOverpaymentDisposition> {
    let flags = [
        parse_bool(fields, "frm1702MX:rdoPg1Pt2I21Refund", errors),
        parse_bool(fields, "frm1702MX:rdoPg1Pt2I21IssueTCC", errors),
        parse_bool(fields, "frm1702MX:rdoPg1Pt2I21CarriedOver", errors),
    ];
    if flags.iter().filter(|flag| **flag == Some(true)).count() > 1 {
        errors.push((
            "part_ii.overpayment_disposition".to_string(),
            "Only one overpayment disposition may be selected".to_string(),
        ));
    }
    if flags[0] == Some(true) {
        Some(Form1702MXOverpaymentDisposition::Refund)
    } else if flags[1] == Some(true) {
        Some(Form1702MXOverpaymentDisposition::TaxCreditCertificate)
    } else if flags[2] == Some(true) {
        Some(Form1702MXOverpaymentDisposition::CarryOver)
    } else {
        None
    }
}

fn parse_money(
    fields: &BTreeMap<String, String>,
    key: &str,
    errors: &mut Vec<(String, String)>,
) -> WholePesoInput {
    let raw = field(fields, key).to_string();
    if raw.trim().is_empty() {
        return WholePesoInput::blank();
    }
    let normalized = raw.trim().replace(',', "");
    let whole = if let Some((whole, fraction)) = normalized.split_once('.') {
        if !fraction.chars().all(|character| character == '0') {
            errors.push((
                key.to_string(),
                "1702MX amounts must be whole pesos; non-zero centavos are not allowed".to_string(),
            ));
            None
        } else {
            whole.parse::<i64>().ok()
        }
    } else {
        normalized.parse::<i64>().ok()
    };
    match whole {
        Some(value) => WholePesoInput {
            amount: Some(WholePeso(value)),
            raw,
        },
        None => {
            errors.push((
                key.to_string(),
                "Expected a signed whole-peso amount or a blank value".to_string(),
            ));
            WholePesoInput { amount: None, raw }
        }
    }
}

fn parse_percent(
    fields: &BTreeMap<String, String>,
    key: &str,
    errors: &mut Vec<(String, String)>,
) -> PercentInput {
    let raw = field(fields, key).to_string();
    if raw.trim().is_empty() {
        return PercentInput::blank();
    }
    let normalized = raw.trim();
    let parsed = normalized.parse::<f64>().ok().and_then(|value| {
        let scaled = value * 100.0;
        ((scaled - scaled.round()).abs() < f64::EPSILON && (0.0..=100.0).contains(&value))
            .then_some(scaled.round() as i32)
    });
    match parsed {
        Some(hundredths) => PercentInput {
            hundredths: Some(hundredths),
            raw,
        },
        None => {
            errors.push((
                key.to_string(),
                "Expected a percentage from 0 through 100 with at most two decimals".to_string(),
            ));
            PercentInput {
                hundredths: None,
                raw,
            }
        }
    }
}

fn parse_exclusive_pair(
    fields: &BTreeMap<String, String>,
    first_key: &str,
    second_key: &str,
    field_name: &str,
    errors: &mut Vec<(String, String)>,
) -> Option<bool> {
    match (
        parse_bool(fields, first_key, errors),
        parse_bool(fields, second_key, errors),
    ) {
        (Some(true), Some(false)) => Some(true),
        (Some(false), Some(true)) => Some(false),
        (Some(first), Some(second)) => {
            errors.push((
                field_name.to_string(),
                format!("Expected one selected choice, found {first}/{second}"),
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
    match field(fields, key) {
        "true" => Some(true),
        "false" => Some(false),
        value => {
            errors.push((
                key.to_string(),
                format!("Expected true or false, found {value:?}"),
            ));
            None
        }
    }
}

fn parse_u8(
    fields: &BTreeMap<String, String>,
    key: &str,
    errors: &mut Vec<(String, String)>,
) -> u8 {
    match field(fields, key).parse::<u8>() {
        Ok(value) => value,
        Err(_) => {
            errors.push((key.to_string(), "Expected an unsigned integer".to_string()));
            0
        }
    }
}

fn parse_taxable_year(
    fields: &BTreeMap<String, String>,
    key: &str,
    errors: &mut Vec<(String, String)>,
) -> u16 {
    match field(fields, key).parse::<u16>() {
        Ok(value @ 0..=99) => 2000 + value,
        Ok(value @ 1900..=2200) => value,
        _ => {
            errors.push((
                key.to_string(),
                "Expected a two-digit reviewed year or a four-digit supported year".to_string(),
            ));
            0
        }
    }
}

fn split_tin(tin: &str) -> (String, String, String, String) {
    let digits = tin
        .chars()
        .filter(|character| character.is_ascii_digit())
        .collect::<String>();
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

fn require_exact(
    fields: &BTreeMap<String, String>,
    key: &str,
    expected: &str,
    errors: &mut Vec<(String, String)>,
) {
    if field(fields, key) != expected {
        errors.push((
            key.to_string(),
            format!("Expected reviewed value {expected:?}"),
        ));
    }
}

fn verify_equal(
    fields: &BTreeMap<String, String>,
    key: &str,
    expected: &str,
    errors: &mut Vec<(String, String)>,
) {
    if field(fields, key) != expected {
        errors.push((
            key.to_string(),
            "Repeated identity field does not match the page-one value".to_string(),
        ));
    }
}

fn field<'a>(fields: &'a BTreeMap<String, String>, key: &str) -> &'a str {
    fields.get(key).map(String::as_str).unwrap_or("")
}

fn insert(map: &mut BTreeMap<String, String>, key: &str, value: impl Into<String>) {
    map.insert(key.to_string(), value.into());
}

fn insert_bool(map: &mut BTreeMap<String, String>, key: &str, value: bool) {
    insert(map, key, if value { "true" } else { "false" });
}

fn insert_money(map: &mut BTreeMap<String, String>, key: &str, value: &WholePesoInput) {
    insert(map, key, value.raw.clone());
}

fn insert_percent(map: &mut BTreeMap<String, String>, key: &str, value: &PercentInput) {
    insert(map, key, value.raw.clone());
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use sha2::{Digest, Sha256};

    use super::*;

    #[test]
    fn exact_contract_has_210_unique_fields() {
        let unique = EXPECTED_SOURCE_KEYS
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        assert_eq!(unique.len(), EXACT_SOURCE_FIELD_COUNT);
    }

    #[test]
    fn blank_and_zero_money_inputs_remain_distinct() {
        let mut fields = EXPECTED_SOURCE_KEYS
            .iter()
            .map(|key| ((*key).to_string(), String::new()))
            .collect::<BTreeMap<_, _>>();
        fields.insert("blank".to_string(), String::new());
        fields.insert("zero".to_string(), "0.00".to_string());
        let mut errors = Vec::new();
        let blank = parse_money(&fields, "blank", &mut errors);
        let zero = parse_money(&fields, "zero", &mut errors);
        assert_eq!((blank.amount, zero.amount), (None, Some(WholePeso(0))));
    }

    #[test]
    fn reviewed_source_hash_and_exact_field_round_trip() {
        let Ok(source_dir) = std::env::var("EBIRFORMS_1702MX_SOURCE_DIR") else {
            eprintln!("EBIRFORMS_1702MX_SOURCE_DIR not set; skipping external-source test");
            return;
        };
        let source_dir = Path::new(&source_dir);
        let editable_path = source_dir.join("00000000000000-1702MXv2018C-1225.xml");
        let encrypted_path =
            source_dir.join("00000000000000-1702MXv2018C-1225#TAXPAYER@EXAMPLE.COM#.xml");
        let official_path = source_dir.join("1702-MX Jan 2018 ENCS Final with OSDv2.pdf");
        let attachment_path = source_dir.join("1702-MX Attachment Jan 2018 ENCS Final4.pdf");

        let editable = fs::read(&editable_path).expect("read editable source");
        assert_eq!(
            sha256(&editable),
            super::super::form_1702mx::REVIEWED_EDITABLE_XML_SHA256
        );
        let xml = String::from_utf8(editable).expect("editable source is UTF-8");
        let fields = crate::bir_xml::parse_bir_xml_checked(&xml).expect("parse source");
        assert_eq!(fields.len(), EXACT_SOURCE_FIELD_COUNT);
        let draft = Form1702MXDraft::from_bir_field_map(&fields).expect("import exact source");
        assert_eq!(draft.to_bir_field_map(), fields);
        let regenerated = crate::bir_xml::parse_bir_xml_with_codec_checked(
            &draft.to_bir_xml_payload(),
            bir_rules::serialization::BodyCodec::Utf8PercentRfc3986Unreserved,
        )
        .expect("parse regenerated UTF-8 payload");
        assert_eq!(regenerated, fields);

        assert_eq!(
            sha256(&fs::read(encrypted_path).expect("read encrypted companion")),
            super::super::form_1702mx::REVIEWED_ENCRYPTED_XML_SHA256
        );
        assert_eq!(
            sha256(&fs::read(official_path).expect("read official return")),
            super::super::form_1702mx::OFFICIAL_FORM_SHA256
        );
        assert_eq!(
            sha256(&fs::read(attachment_path).expect("read official attachment")),
            super::super::form_1702mx::OFFICIAL_ATTACHMENT_SHA256
        );
    }

    fn sha256(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }
}
