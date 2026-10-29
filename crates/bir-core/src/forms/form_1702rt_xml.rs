//! Checked 258-field editable-save contract for exact form `1702RTv2018C`.
//!
//! The plain save and decrypted encrypted companion share the same field set.
//! `txtFinalFlag` is `1` in the editable save and `0` in the encrypted copy.
//! Import does not silently repair inconsistent stored calculations: it keeps
//! all values losslessly, while `FormValidator` compares them to the formulas
//! printed on the locked official PDF.

use std::collections::{BTreeMap, BTreeSet};

use super::FormValidator;
use super::form_1702rt::{
    Form1702RTAtcSelection, Form1702RTDate, Form1702RTDeductionMethod, Form1702RTDraft,
    Form1702RTFilingBasis, Form1702RTMcitRow, Form1702RTNamedAmount, Form1702RTNolcoRow,
    Form1702RTOverpaymentDisposition, Form1702RTPartII, Form1702RTPartIV, Form1702RTPartV,
    Form1702RTPaymentDetail, Form1702RTSchedule1, Form1702RTSchedule2, Form1702RTSchedule3,
    Form1702RTSchedule4, Form1702RTSchedule5, Form1702RTSpecialDeductionRow, Form1702RTTaxCredits,
    WholePeso,
};

const EXACT_SOURCE_FIELD_COUNT: usize = 258;

impl Form1702RTDraft {
    /// Serialize the exact reviewed field union without mutating or silently
    /// recomputing an imported save. Call `try_to_bir_xml_payload` when a
    /// formula-consistent editable-save payload is required.
    pub fn to_bir_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = self.preserved_transport_fields.clone();
        let (tin1, tin2, tin3, branch) = split_tin(&self.tin);

        insert_bool(
            &mut fields,
            "frm1702RT:rdoPg1I1Calendar",
            matches!(self.filing_basis, Form1702RTFilingBasis::Calendar),
        );
        insert_bool(
            &mut fields,
            "frm1702RT:rdoPg1I1Fiscal",
            matches!(self.filing_basis, Form1702RTFilingBasis::Fiscal),
        );
        insert(
            &mut fields,
            "frm1702RT:ddlPg1I2Month",
            format!("{:02}", self.month),
        );
        insert(
            &mut fields,
            "frm1702RT:txtPg1I2Year",
            format!("{:02}", self.taxable_year % 100),
        );
        insert_bool(&mut fields, "frm1702RT:rdoPg1I3AmmendYes", self.is_amended);
        insert_bool(&mut fields, "frm1702RT:rdoPg1I3AmmendNo", !self.is_amended);
        insert_bool(
            &mut fields,
            "frm1702RT:rdoPg1I4ShortPeriodYes",
            self.is_short_period,
        );
        insert_bool(
            &mut fields,
            "frm1702RT:rdoPg1I4ShortPeriodNo",
            !self.is_short_period,
        );
        insert_bool(
            &mut fields,
            "frm1702RT:rdoPg1I5Atc",
            self.atc.printed_mcit_selected,
        );
        insert(
            &mut fields,
            "frm1702RT:drpPg1I5AtcOther",
            self.atc.other_code.clone(),
        );
        insert_bool(
            &mut fields,
            "frm1702RT:rdoPg1I5AtcOther",
            self.atc.other_selected,
        );
        insert(&mut fields, "frm1702RT:txtPg1Pt1I6TIN1", tin1.clone());
        insert(&mut fields, "frm1702RT:txtPg1Pt1I6TIN2", tin2.clone());
        insert(&mut fields, "frm1702RT:txtPg1Pt1I6TIN3", tin3.clone());
        insert(&mut fields, "frm1702RT:txtPg1Pt1I6TIN4", branch.clone());
        insert(&mut fields, "BranchMaskP1", branch.clone());
        insert(&mut fields, "frm1702RT:txtRDO", self.rdo_code.clone());
        insert(
            &mut fields,
            "frm1702RT:drpPg1Pt1I7RDOCode",
            self.rdo_code.clone(),
        );
        for (index, value) in self.registered_name_lines.iter().enumerate() {
            insert(
                &mut fields,
                &format!("frm1702RT:txtPg1Pt1I8Name{}", index + 1),
                value.clone(),
            );
        }
        for (index, value) in self.registered_address_lines.iter().enumerate() {
            insert(
                &mut fields,
                &format!("frm1702RT:txtPg1Pt1I9Address{}", index + 1),
                value.clone(),
            );
        }
        insert(&mut fields, "frm1702RT:txtZIP", self.zip_code.clone());
        insert_optional_date(
            &mut fields,
            "frm1702RT:txtPg1Pt1I10",
            self.incorporation_date,
        );
        insert(
            &mut fields,
            "frm1702RT:txtPg1Pt1I11Contact",
            self.contact_number.clone(),
        );
        insert(
            &mut fields,
            "frm1702RT:txtPg1Pt1I12Email",
            self.email.clone(),
        );
        insert_bool(
            &mut fields,
            "frm1702RT:rdoPg1Pt1I13ItemizedDeduction",
            matches!(self.deduction_method, Form1702RTDeductionMethod::Itemized),
        );
        insert_bool(
            &mut fields,
            "frm1702RT:rdoPg1Pt1I13OptionalStandard",
            matches!(
                self.deduction_method,
                Form1702RTDeductionMethod::OptionalStandard
            ),
        );

        let p2 = &self.part_ii;
        for (key, value) in [
            ("frm1702RT:txtPg1Pt2I14IncomeTax", p2.item_14_tax_due),
            (
                "frm1702RT:txtPg1Pt2I15TotalTaxCredits",
                p2.item_15_total_tax_credits,
            ),
            (
                "frm1702RT:txtPg1Pt2I16NetTax",
                p2.item_16_net_tax_payable_or_overpayment,
            ),
            ("frm1702RT:txtPg1Pt2I17Surcharge", p2.item_17_surcharge),
            ("frm1702RT:txtPg1Pt2I18Interest", p2.item_18_interest),
            ("frm1702RT:txtPg1Pt2I19Compromise", p2.item_19_compromise),
            (
                "frm1702RT:txtPg1Pt2I20TotalPenalties",
                p2.item_20_total_penalties,
            ),
            (
                "frm1702RT:txtPg1Pt2I21TotalAmount",
                p2.item_21_total_amount_payable_or_overpayment,
            ),
        ] {
            insert_money(&mut fields, key, value);
        }
        insert_bool(
            &mut fields,
            "frm1702RT:rdoPg1Pt2I21OverpaymentRefunded",
            matches!(
                p2.overpayment_disposition,
                Some(Form1702RTOverpaymentDisposition::Refund)
            ),
        );
        insert_bool(
            &mut fields,
            "frm1702RT:rdoPg1Pt2I21OverpaymentIssued",
            matches!(
                p2.overpayment_disposition,
                Some(Form1702RTOverpaymentDisposition::TaxCreditCertificate)
            ),
        );
        insert_bool(
            &mut fields,
            "frm1702RT:rdoPg1Pt2I21OverpaymentCarried",
            matches!(
                p2.overpayment_disposition,
                Some(Form1702RTOverpaymentDisposition::CarryOver)
            ),
        );
        insert(
            &mut fields,
            "frm1702RT:txtSignaturePresident",
            self.president_signature.clone(),
        );
        insert(
            &mut fields,
            "frm1702RT:txtSignatureTreasurer",
            self.treasurer_signature.clone(),
        );
        insert(
            &mut fields,
            "frm1702RT:txtPg1Pt2PagesFilled",
            self.number_of_attachments.clone(),
        );
        insert(
            &mut fields,
            "frm1702RT:txtPg1Pt2Signatory1",
            self.president_signatory_title.clone(),
        );
        insert(
            &mut fields,
            "frm1702RT:txtPg1Pt2SignatoryTin1",
            self.president_signatory_tin.clone(),
        );
        insert(
            &mut fields,
            "frm1702RT:txtPg1Pt2Signatory2",
            self.treasurer_signatory_title.clone(),
        );
        insert(
            &mut fields,
            "frm1702RT:txtPg1Pt2SignatoryTin2",
            self.treasurer_signatory_tin.clone(),
        );

        insert_payment_rows(&mut fields, &self.payment_details);
        for page in 2..=4 {
            insert(
                &mut fields,
                &format!("frm1702RT:txtPg{page}TIN1"),
                tin1.clone(),
            );
            insert(
                &mut fields,
                &format!("frm1702RT:txtPg{page}TIN2"),
                tin2.clone(),
            );
            insert(
                &mut fields,
                &format!("frm1702RT:txtPg{page}TIN3"),
                tin3.clone(),
            );
            insert(
                &mut fields,
                &format!("frm1702RT:txtPg{page}TIN4"),
                branch.clone(),
            );
            insert(
                &mut fields,
                &format!("txtBranchMaskP{page}"),
                branch.clone(),
            );
            insert(
                &mut fields,
                &format!("frm1702RT:txtPg{page}RegisteredName"),
                self.taxpayer_name.clone(),
            );
        }

        insert_part_iv(&mut fields, &self.part_iv);
        insert_part_v(&mut fields, &self.part_v);
        insert_schedule_1(&mut fields, &self.schedule_1);
        insert_schedule_2(&mut fields, &self.schedule_2);
        insert_schedule_3(&mut fields, &self.schedule_3);
        insert_schedule_4(&mut fields, &self.schedule_4);
        insert_schedule_5(&mut fields, &self.schedule_5);
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
                "xml_payload".to_string(),
                format!("Invalid 1702RT pseudo-XML: {error}"),
            )]
        })?;
        Self::from_bir_field_map(&fields)
    }
}

fn parse_part_ii(
    fields: &BTreeMap<String, String>,
    disposition: Option<Form1702RTOverpaymentDisposition>,
    errors: &mut Vec<(String, String)>,
) -> Form1702RTPartII {
    Form1702RTPartII {
        item_14_tax_due: parse_money(fields, "frm1702RT:txtPg1Pt2I14IncomeTax", errors),
        item_15_total_tax_credits: parse_money(
            fields,
            "frm1702RT:txtPg1Pt2I15TotalTaxCredits",
            errors,
        ),
        item_16_net_tax_payable_or_overpayment: parse_money(
            fields,
            "frm1702RT:txtPg1Pt2I16NetTax",
            errors,
        ),
        item_17_surcharge: parse_money(fields, "frm1702RT:txtPg1Pt2I17Surcharge", errors),
        item_18_interest: parse_money(fields, "frm1702RT:txtPg1Pt2I18Interest", errors),
        item_19_compromise: parse_money(fields, "frm1702RT:txtPg1Pt2I19Compromise", errors),
        item_20_total_penalties: parse_money(
            fields,
            "frm1702RT:txtPg1Pt2I20TotalPenalties",
            errors,
        ),
        item_21_total_amount_payable_or_overpayment: parse_money(
            fields,
            "frm1702RT:txtPg1Pt2I21TotalAmount",
            errors,
        ),
        overpayment_disposition: disposition,
    }
}

fn parse_payment_rows(
    fields: &BTreeMap<String, String>,
    errors: &mut Vec<(String, String)>,
) -> [Form1702RTPaymentDetail; 4] {
    let mut rows: [Form1702RTPaymentDetail; 4] =
        std::array::from_fn(|_| Form1702RTPaymentDetail::default());
    for (index, prefix) in [
        "frm1702RT:txtPg1Pt3I23DebitMemo",
        "frm1702RT:txtPg1Pt3I24Check",
    ]
    .into_iter()
    .enumerate()
    {
        rows[index] = Form1702RTPaymentDetail {
            drawee_bank_or_agency: field(fields, &format!("{prefix}C1")).to_string(),
            number: field(fields, &format!("{prefix}C2")).to_string(),
            date: parse_optional_date(fields, &format!("{prefix}C3Date"), errors),
            amount: parse_money(fields, &format!("{prefix}C4Amount"), errors),
            ..Form1702RTPaymentDetail::default()
        };
    }
    rows[2] = Form1702RTPaymentDetail {
        number: field(fields, "frm1702RT:txtPg1Pt3I25TaxDebitC2").to_string(),
        date: parse_optional_date(fields, "frm1702RT:txtPg1Pt3I25TaxDebitDate", errors),
        amount: parse_money(fields, "frm1702RT:txtPg1Pt3I25TaxDebitC4Amount", errors),
        ..Form1702RTPaymentDetail::default()
    };
    rows[3] = Form1702RTPaymentDetail {
        specification: field(fields, "frm1702RT:txtPg1Pt3I26Others").to_string(),
        drawee_bank_or_agency: field(fields, "frm1702RT:txtPg1Pt3I26OthersC1").to_string(),
        number: field(fields, "frm1702RT:txtPg1Pt3I26OthersC2").to_string(),
        date: parse_optional_date(fields, "frm1702RT:txtPg1Pt3I26OthersC3Date", errors),
        amount: parse_money(fields, "frm1702RT:txtPg1Pt3I26OthersC4Amount", errors),
    };
    rows
}

fn parse_part_iv(
    fields: &BTreeMap<String, String>,
    errors: &mut Vec<(String, String)>,
) -> Form1702RTPartIV {
    Form1702RTPartIV {
        item_27_sales: parse_money(fields, "frm1702RT:txtPg2Pt4I27Sales", errors),
        item_28_sales_returns: parse_money(fields, "frm1702RT:txtPg2Pt4I28LessSales", errors),
        item_29_net_sales: parse_money(fields, "frm1702RT:txtPg2Pt4I29NetSales", errors),
        item_30_cost_of_sales_or_services: parse_money(
            fields,
            "frm1702RT:txtPg2Pt4I30LessCost",
            errors,
        ),
        item_31_gross_income_from_operations: parse_money(
            fields,
            "frm1702RT:txtPg2Pt4I31GrossIncome",
            errors,
        ),
        item_32_other_taxable_income: parse_money(
            fields,
            "frm1702RT:txtPg2Pt4I32AddOtherTaxable",
            errors,
        ),
        item_33_total_taxable_income: parse_money(
            fields,
            "frm1702RT:txtPg2Pt4I33TotalGross",
            errors,
        ),
        item_34_ordinary_itemized_deductions: parse_money(
            fields,
            "frm1702RT:txtPg2Pt4I34OrdinaryAllowable",
            errors,
        ),
        item_35_special_itemized_deductions: parse_money(
            fields,
            "frm1702RT:txtPg2Pt4I35SpecialAllowable",
            errors,
        ),
        item_36_nolco: parse_money(fields, "frm1702RT:txtPg2Pt4I36Nolco", errors),
        item_37_total_itemized_deductions: parse_money(
            fields,
            "frm1702RT:txtPg2Pt4I37TotalItemized",
            errors,
        ),
        item_38_optional_standard_deduction: parse_money(
            fields,
            "frm1702RT:txtPg2Pt4I38OptionalStandard",
            errors,
        ),
        item_39_net_taxable_income_or_loss: parse_money(
            fields,
            "frm1702RT:txtPg2Pt4I39NetTaxable",
            errors,
        ),
        item_40_income_tax_rate_percent: parse_u8(
            fields,
            "frm1702RT:Pg2Pt4I40IncomeTaxRate",
            errors,
        ),
        item_41_normal_income_tax_due: parse_money(
            fields,
            "frm1702RT:txtPg2Pt4I41IncomeTaxDue",
            errors,
        ),
        item_42_mcit_due: parse_money(fields, "frm1702RT:txtPg2Pt4I42MinimumCorporate", errors),
        item_43_tax_due: parse_money(fields, "frm1702RT:txtPg2Pt4I43TotalIncomeTax", errors),
        tax_credits: Form1702RTTaxCredits {
            item_44_prior_year_excess_credits: parse_money(
                fields,
                "frm1702RT:txtPg2Pt4I44ExcessCredits",
                errors,
            ),
            item_45_previous_quarter_mcit_payments: parse_money(
                fields,
                "frm1702RT:txtPg2Pt4I45IncomeTaxPaymentUnderMCIT",
                errors,
            ),
            item_46_previous_quarter_regular_payments: parse_money(
                fields,
                "frm1702RT:txtPg2Pt4I46IncomeTaxUnderRegular",
                errors,
            ),
            item_47_excess_mcit_applied: parse_money(
                fields,
                "frm1702RT:txtPg2Pt4I47ExcessMCIT",
                errors,
            ),
            item_48_previous_quarter_withholding: parse_money(
                fields,
                "frm1702RT:txtPg2Pt4I48CreditableTaxWithheldFromPrevious",
                errors,
            ),
            item_49_fourth_quarter_withholding: parse_money(
                fields,
                "frm1702RT:txtPg2Pt4I49CreditableTaxWithheldFor4thQuarter",
                errors,
            ),
            item_50_foreign_tax_credits: parse_money(
                fields,
                "frm1702RT:txtPg2Pt4I50ForeignTaxCredits",
                errors,
            ),
            item_51_tax_paid_on_previous_return: parse_money(
                fields,
                "frm1702RT:txtPg2Pt4I51TaxPaidInReturn",
                errors,
            ),
            item_52_special_tax_credits: parse_money(
                fields,
                "frm1702RT:txtPg2Pt452SpecialTaxCredits",
                errors,
            ),
            item_53_other: Form1702RTNamedAmount {
                description: field(fields, "frm1702RT:txtPg2Pt4I53C1").to_string(),
                amount: parse_money(fields, "frm1702RT:txtPg2Pt4I53C2", errors),
            },
            item_54_other: Form1702RTNamedAmount {
                description: field(fields, "frm1702RT:txtPg2Pt4I54C1").to_string(),
                amount: parse_money(fields, "frm1702RT:txtPg2Pt4I54C2", errors),
            },
            item_55_total: parse_money(fields, "frm1702RT:txtPg2Pt4I55TotalTaxCredits", errors),
        },
        item_56_net_tax_payable_or_overpayment: parse_money(
            fields,
            "frm1702RT:txtPg2Pt4I56NetTax",
            errors,
        ),
    }
}

fn parse_part_v(
    fields: &BTreeMap<String, String>,
    errors: &mut Vec<(String, String)>,
) -> Form1702RTPartV {
    Form1702RTPartV {
        item_57_special_allowable_deductions_tax_effect: parse_money(
            fields,
            "frm1702RT:txtPg2Pt5I57SpecialAllowable",
            errors,
        ),
        item_58_special_tax_credits: parse_money(
            fields,
            "frm1702RT:txtPg2Pt5I58AddSpecialTax",
            errors,
        ),
        item_59_total_tax_relief: parse_money(fields, "frm1702RT:txtPg2Pt5I59TotalTax", errors),
    }
}

fn parse_schedule_1(
    fields: &BTreeMap<String, String>,
    errors: &mut Vec<(String, String)>,
) -> Form1702RTSchedule1 {
    let other = std::array::from_fn(|index| {
        let suffix = char::from(b'd' + u8::try_from(index).unwrap_or(0));
        Form1702RTNamedAmount {
            description: field(fields, &format!("frm1702RT:txtPg3Sc1I17{suffix}C1")).to_string(),
            amount: parse_money(fields, &format!("frm1702RT:txtPg3Sc1I17{suffix}C2"), errors),
        }
    });
    Form1702RTSchedule1 {
        amortizations: parse_money(fields, "frm1702RT:txtPg3Sc1I1Amortization", errors),
        bad_debts: parse_money(fields, "frm1702RT:txtPg3Sc1I2BadDebts", errors),
        charitable_contributions: parse_money(
            fields,
            "frm1702RT:txtPg3Sc1I3CharitableContributions",
            errors,
        ),
        depletion: parse_money(fields, "frm1702RT:txtPg3Sc1I4Depletion", errors),
        depreciation: parse_money(fields, "frm1702RT:txtPg3Sc1I5Depreciation", errors),
        entertainment: parse_money(fields, "frm1702RT:txtPg3Sc1I6Entertainment", errors),
        fringe_benefits: parse_money(fields, "frm1702RT:txtPg3Sc1I7FringeBenefits", errors),
        interest: parse_money(fields, "frm1702RT:txtPg3Sc1I8Interest", errors),
        losses: parse_money(fields, "frm1702RT:txtPg3Sc1I9Losses", errors),
        pension_trusts: parse_money(fields, "frm1702RT:txtPg3Sc1I10PensionTrust", errors),
        rental: parse_money(fields, "frm1702RT:txtPg3Sc1I11Rental", errors),
        research_and_development: parse_money(fields, "frm1702RT:txtPg3Sc1I12Research", errors),
        salaries_wages_allowances: parse_money(fields, "frm1702RT:txtPg3Sc1I13Salaries", errors),
        statutory_contributions: parse_money(fields, "frm1702RT:txtPg3Sc1I14Contributions", errors),
        taxes_and_licenses: parse_money(fields, "frm1702RT:txtPg3Sc1I15TaxesandLicenses", errors),
        transportation_and_travel: parse_money(
            fields,
            "frm1702RT:txtPg3Sc1I16TransportationandTravel",
            errors,
        ),
        janitorial_and_messengerial: parse_money(
            fields,
            "frm1702RT:txtPg3Sc1I17aJanitorial",
            errors,
        ),
        professional_fees: parse_money(fields, "frm1702RT:txtPg3Sc1I17bProfessionalFees", errors),
        security_services: parse_money(fields, "frm1702RT:txtPg3Sc1I17cSecurityServices", errors),
        other,
        item_18_total: parse_money(
            fields,
            "frm1702RT:txtPg3Sc1I18TotalOrdinaryAllowable",
            errors,
        ),
    }
}

fn parse_schedule_2(
    fields: &BTreeMap<String, String>,
    errors: &mut Vec<(String, String)>,
) -> Form1702RTSchedule2 {
    Form1702RTSchedule2 {
        rows: std::array::from_fn(|index| {
            let item = index + 1;
            Form1702RTSpecialDeductionRow {
                description: field(fields, &format!("frm1702RT:txtPg3Sc2I{item}C1")).to_string(),
                legal_basis: field(fields, &format!("frm1702RT:txtPg3Sc2I{item}C2")).to_string(),
                amount: parse_money(fields, &format!("frm1702RT:txtPg3Sc2I{item}C3"), errors),
            }
        }),
        item_5_total: parse_money(fields, "frm1702RT:txtPg3Sc2I5TotalSpecialAllowable", errors),
    }
}

fn parse_schedule_3(
    fields: &BTreeMap<String, String>,
    errors: &mut Vec<(String, String)>,
) -> Form1702RTSchedule3 {
    Form1702RTSchedule3 {
        item_1_gross_income: parse_money(fields, "frm1702RT:txtPg4Sc3I1GrossIncome", errors),
        item_2_ordinary_deductions: parse_money(
            fields,
            "frm1702RT:txtPg4Sc3I2TotalDeductions",
            errors,
        ),
        item_3_net_operating_loss: parse_money(
            fields,
            "frm1702RT:txtPg4Sc3I3NetOperatingLoss",
            errors,
        ),
        rows: std::array::from_fn(|index| {
            let item = index + 4;
            Form1702RTNolcoRow {
                year_incurred: field(fields, &format!("frm1702RT:txtPg4Sc3AI{item}C1")).to_string(),
                amount: parse_money(fields, &format!("frm1702RT:txtPg4Sc3AI{item}C2"), errors),
                applied_previous_years: parse_money(
                    fields,
                    &format!("frm1702RT:txtPg4Sc3AI{item}C3"),
                    errors,
                ),
                expired: parse_money(fields, &format!("frm1702RT:txtPg4Sc3AI{item}C4"), errors),
                applied_current_year: parse_money(
                    fields,
                    &format!("frm1702RT:txtPg4Sc3AI{item}C5"),
                    errors,
                ),
                unapplied_balance: parse_money(
                    fields,
                    &format!("frm1702RT:txtPg4Sc3AI{item}C6"),
                    errors,
                ),
            }
        }),
        item_8_total_applied_current_year: parse_money(
            fields,
            "frm1702RT:txtPg4Sc4I8TotalNOLCO",
            errors,
        ),
    }
}

fn parse_schedule_4(
    fields: &BTreeMap<String, String>,
    errors: &mut Vec<(String, String)>,
) -> Form1702RTSchedule4 {
    Form1702RTSchedule4 {
        rows: std::array::from_fn(|index| {
            let item = index + 1;
            Form1702RTMcitRow {
                year: field(fields, &format!("frm1702RT:txtPg4Sc4I{item}C1")).to_string(),
                normal_income_tax: parse_money(
                    fields,
                    &format!("frm1702RT:txtPg4Sc4I{item}C2"),
                    errors,
                ),
                mcit: parse_money(fields, &format!("frm1702RT:txtPg4Sc4I{item}C3"), errors),
                excess_mcit: parse_money(fields, &format!("frm1702RT:txtPg4Sc4I{item}C4"), errors),
                applied_previous_years: parse_money(
                    fields,
                    &format!("frm1702RT:txtPg4Sc4I{item}C5"),
                    errors,
                ),
                expired: parse_money(fields, &format!("frm1702RT:txtPg4Sc4I{item}C6"), errors),
                applied_current_year: parse_money(
                    fields,
                    &format!("frm1702RT:txtPg4Sc4I{item}C7"),
                    errors,
                ),
                allowable_balance: parse_money(
                    fields,
                    &format!("frm1702RT:txtPg4Sc4I{item}C8"),
                    errors,
                ),
            }
        }),
        item_4_total_applied_current_year: parse_money(
            fields,
            "frm1702RT:txtPg4Sc4I4TotalExcessMCIT",
            errors,
        ),
    }
}

fn parse_schedule_5(
    fields: &BTreeMap<String, String>,
    errors: &mut Vec<(String, String)>,
) -> Form1702RTSchedule5 {
    let parse_named = |item: usize, errors: &mut Vec<(String, String)>| Form1702RTNamedAmount {
        description: field(fields, &format!("frm1702RT:txtPg4Sc5I{item}C1")).to_string(),
        amount: parse_money(fields, &format!("frm1702RT:txtPg4Sc5I{item}C2"), errors),
    };
    Form1702RTSchedule5 {
        item_1_net_income_or_loss_per_books: parse_money(
            fields,
            "frm1702RT:txtPg4Sc5I1NetIncome",
            errors,
        ),
        additions: [parse_named(2, errors), parse_named(3, errors)],
        item_4_total: parse_money(fields, "frm1702RT:txtPg4Sc5I4Total", errors),
        non_taxable_income: [parse_named(5, errors), parse_named(6, errors)],
        special_deductions: [parse_named(7, errors), parse_named(8, errors)],
        item_9_total: parse_money(fields, "frm1702RT:txtPg4Sc5I9Total", errors),
        item_10_net_taxable_income_or_loss: parse_money(
            fields,
            "frm1702RT:txtPg4Sc5I10NetTaxableIncome",
            errors,
        ),
    }
}

fn expected_xml_keys() -> BTreeSet<String> {
    Form1702RTDraft::default()
        .to_bir_field_map()
        .into_keys()
        .collect()
}

fn field<'a>(fields: &'a BTreeMap<String, String>, key: &str) -> &'a str {
    fields.get(key).map(String::as_str).unwrap_or_default()
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

fn parse_exclusive_pair(
    fields: &BTreeMap<String, String>,
    first: &str,
    second: &str,
    semantic_field: &str,
    errors: &mut Vec<(String, String)>,
) -> Option<bool> {
    match (
        parse_bool(fields, first, errors),
        parse_bool(fields, second, errors),
    ) {
        (Some(true), Some(false)) => Some(true),
        (Some(false), Some(true)) => Some(false),
        (Some(left), Some(right)) => {
            errors.push((
                semantic_field.to_string(),
                format!("Expected exactly one option, found {left} and {right}"),
            ));
            None
        }
        _ => None,
    }
}

fn parse_money(
    fields: &BTreeMap<String, String>,
    key: &str,
    errors: &mut Vec<(String, String)>,
) -> WholePeso {
    match WholePeso::parse_bir(field(fields, key)) {
        Ok(value) => value,
        Err(error) => {
            errors.push((
                key.to_string(),
                format!("Invalid whole-peso amount: {error}"),
            ));
            WholePeso::ZERO
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
            errors.push((
                key.to_string(),
                "Expected an unsigned integer from 0 to 255".to_string(),
            ));
            0
        }
    }
}

fn parse_taxable_year(
    fields: &BTreeMap<String, String>,
    key: &str,
    errors: &mut Vec<(String, String)>,
) -> u16 {
    let value = field(fields, key);
    match value.parse::<u16>() {
        Ok(year @ 0..=99) => 2000 + year,
        Ok(year) if (1900..=2200).contains(&year) => year,
        _ => {
            errors.push((
                key.to_string(),
                "Expected a two-digit or four-digit taxable year".to_string(),
            ));
            0
        }
    }
}

fn parse_optional_date(
    fields: &BTreeMap<String, String>,
    key: &str,
    errors: &mut Vec<(String, String)>,
) -> Option<Form1702RTDate> {
    let value = field(fields, key).trim();
    if value.is_empty() {
        return None;
    }
    match Form1702RTDate::parse(value) {
        Ok(value) => Some(value),
        Err(error) => {
            errors.push((key.to_string(), error));
            None
        }
    }
}

fn require_exact(
    fields: &BTreeMap<String, String>,
    key: &str,
    expected: &str,
    errors: &mut Vec<(String, String)>,
) {
    if field(fields, key) != expected {
        errors.push((key.to_string(), format!("Expected {expected:?}")));
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
            format!("Duplicate identity value differs from {expected:?}"),
        ));
    }
}

fn split_tin(tin: &str) -> (String, String, String, String) {
    let digits = tin
        .chars()
        .filter(|character| character.is_ascii_digit())
        .collect::<String>();
    let first = digits.get(0..3).unwrap_or_default().to_string();
    let second = digits.get(3..6).unwrap_or_default().to_string();
    let third = digits.get(6..9).unwrap_or_default().to_string();
    let branch = digits.get(9..).unwrap_or_default().to_string();
    (first, second, third, branch)
}

fn insert(map: &mut BTreeMap<String, String>, key: &str, value: impl Into<String>) {
    map.insert(key.to_string(), value.into());
}

fn insert_bool(map: &mut BTreeMap<String, String>, key: &str, value: bool) {
    insert(map, key, if value { "true" } else { "false" });
}

fn insert_money(map: &mut BTreeMap<String, String>, key: &str, value: WholePeso) {
    insert(map, key, value.format_bir());
}

fn insert_optional_date(
    map: &mut BTreeMap<String, String>,
    key: &str,
    value: Option<Form1702RTDate>,
) {
    insert(
        map,
        key,
        value.map(|date| date.to_string()).unwrap_or_default(),
    );
}

fn insert_payment_rows(fields: &mut BTreeMap<String, String>, rows: &[Form1702RTPaymentDetail; 4]) {
    for (prefix, row) in [
        ("frm1702RT:txtPg1Pt3I23DebitMemo", &rows[0]),
        ("frm1702RT:txtPg1Pt3I24Check", &rows[1]),
    ] {
        insert(
            fields,
            &format!("{prefix}C1"),
            row.drawee_bank_or_agency.clone(),
        );
        insert(fields, &format!("{prefix}C2"), row.number.clone());
        insert_optional_date(fields, &format!("{prefix}C3Date"), row.date);
        insert_money(fields, &format!("{prefix}C4Amount"), row.amount);
    }
    insert(
        fields,
        "frm1702RT:txtPg1Pt3I25TaxDebitC2",
        rows[2].number.clone(),
    );
    insert_optional_date(fields, "frm1702RT:txtPg1Pt3I25TaxDebitDate", rows[2].date);
    insert_money(
        fields,
        "frm1702RT:txtPg1Pt3I25TaxDebitC4Amount",
        rows[2].amount,
    );
    insert(
        fields,
        "frm1702RT:txtPg1Pt3I26Others",
        rows[3].specification.clone(),
    );
    insert(
        fields,
        "frm1702RT:txtPg1Pt3I26OthersC1",
        rows[3].drawee_bank_or_agency.clone(),
    );
    insert(
        fields,
        "frm1702RT:txtPg1Pt3I26OthersC2",
        rows[3].number.clone(),
    );
    insert_optional_date(fields, "frm1702RT:txtPg1Pt3I26OthersC3Date", rows[3].date);
    insert_money(
        fields,
        "frm1702RT:txtPg1Pt3I26OthersC4Amount",
        rows[3].amount,
    );
}

fn insert_part_iv(fields: &mut BTreeMap<String, String>, part: &Form1702RTPartIV) {
    for (key, value) in [
        ("frm1702RT:txtPg2Pt4I27Sales", part.item_27_sales),
        (
            "frm1702RT:txtPg2Pt4I28LessSales",
            part.item_28_sales_returns,
        ),
        ("frm1702RT:txtPg2Pt4I29NetSales", part.item_29_net_sales),
        (
            "frm1702RT:txtPg2Pt4I30LessCost",
            part.item_30_cost_of_sales_or_services,
        ),
        (
            "frm1702RT:txtPg2Pt4I31GrossIncome",
            part.item_31_gross_income_from_operations,
        ),
        (
            "frm1702RT:txtPg2Pt4I32AddOtherTaxable",
            part.item_32_other_taxable_income,
        ),
        (
            "frm1702RT:txtPg2Pt4I33TotalGross",
            part.item_33_total_taxable_income,
        ),
        (
            "frm1702RT:txtPg2Pt4I34OrdinaryAllowable",
            part.item_34_ordinary_itemized_deductions,
        ),
        (
            "frm1702RT:txtPg2Pt4I35SpecialAllowable",
            part.item_35_special_itemized_deductions,
        ),
        ("frm1702RT:txtPg2Pt4I36Nolco", part.item_36_nolco),
        (
            "frm1702RT:txtPg2Pt4I37TotalItemized",
            part.item_37_total_itemized_deductions,
        ),
        (
            "frm1702RT:txtPg2Pt4I38OptionalStandard",
            part.item_38_optional_standard_deduction,
        ),
        (
            "frm1702RT:txtPg2Pt4I39NetTaxable",
            part.item_39_net_taxable_income_or_loss,
        ),
        (
            "frm1702RT:txtPg2Pt4I41IncomeTaxDue",
            part.item_41_normal_income_tax_due,
        ),
        (
            "frm1702RT:txtPg2Pt4I42MinimumCorporate",
            part.item_42_mcit_due,
        ),
        ("frm1702RT:txtPg2Pt4I43TotalIncomeTax", part.item_43_tax_due),
        (
            "frm1702RT:txtPg2Pt4I44ExcessCredits",
            part.tax_credits.item_44_prior_year_excess_credits,
        ),
        (
            "frm1702RT:txtPg2Pt4I45IncomeTaxPaymentUnderMCIT",
            part.tax_credits.item_45_previous_quarter_mcit_payments,
        ),
        (
            "frm1702RT:txtPg2Pt4I46IncomeTaxUnderRegular",
            part.tax_credits.item_46_previous_quarter_regular_payments,
        ),
        (
            "frm1702RT:txtPg2Pt4I47ExcessMCIT",
            part.tax_credits.item_47_excess_mcit_applied,
        ),
        (
            "frm1702RT:txtPg2Pt4I48CreditableTaxWithheldFromPrevious",
            part.tax_credits.item_48_previous_quarter_withholding,
        ),
        (
            "frm1702RT:txtPg2Pt4I49CreditableTaxWithheldFor4thQuarter",
            part.tax_credits.item_49_fourth_quarter_withholding,
        ),
        (
            "frm1702RT:txtPg2Pt4I50ForeignTaxCredits",
            part.tax_credits.item_50_foreign_tax_credits,
        ),
        (
            "frm1702RT:txtPg2Pt4I51TaxPaidInReturn",
            part.tax_credits.item_51_tax_paid_on_previous_return,
        ),
        (
            "frm1702RT:txtPg2Pt452SpecialTaxCredits",
            part.tax_credits.item_52_special_tax_credits,
        ),
        (
            "frm1702RT:txtPg2Pt4I53C2",
            part.tax_credits.item_53_other.amount,
        ),
        (
            "frm1702RT:txtPg2Pt4I54C2",
            part.tax_credits.item_54_other.amount,
        ),
        (
            "frm1702RT:txtPg2Pt4I55TotalTaxCredits",
            part.tax_credits.item_55_total,
        ),
        (
            "frm1702RT:txtPg2Pt4I56NetTax",
            part.item_56_net_tax_payable_or_overpayment,
        ),
    ] {
        insert_money(fields, key, value);
    }
    insert(
        fields,
        "frm1702RT:Pg2Pt4I40IncomeTaxRate",
        part.item_40_income_tax_rate_percent.to_string(),
    );
    insert(
        fields,
        "frm1702RT:txtPg2Pt4I53C1",
        part.tax_credits.item_53_other.description.clone(),
    );
    insert(
        fields,
        "frm1702RT:txtPg2Pt4I54C1",
        part.tax_credits.item_54_other.description.clone(),
    );
}

fn insert_part_v(fields: &mut BTreeMap<String, String>, part: &Form1702RTPartV) {
    insert_money(
        fields,
        "frm1702RT:txtPg2Pt5I57SpecialAllowable",
        part.item_57_special_allowable_deductions_tax_effect,
    );
    insert_money(
        fields,
        "frm1702RT:txtPg2Pt5I58AddSpecialTax",
        part.item_58_special_tax_credits,
    );
    insert_money(
        fields,
        "frm1702RT:txtPg2Pt5I59TotalTax",
        part.item_59_total_tax_relief,
    );
}

fn insert_schedule_1(fields: &mut BTreeMap<String, String>, schedule: &Form1702RTSchedule1) {
    let keys = [
        "frm1702RT:txtPg3Sc1I1Amortization",
        "frm1702RT:txtPg3Sc1I2BadDebts",
        "frm1702RT:txtPg3Sc1I3CharitableContributions",
        "frm1702RT:txtPg3Sc1I4Depletion",
        "frm1702RT:txtPg3Sc1I5Depreciation",
        "frm1702RT:txtPg3Sc1I6Entertainment",
        "frm1702RT:txtPg3Sc1I7FringeBenefits",
        "frm1702RT:txtPg3Sc1I8Interest",
        "frm1702RT:txtPg3Sc1I9Losses",
        "frm1702RT:txtPg3Sc1I10PensionTrust",
        "frm1702RT:txtPg3Sc1I11Rental",
        "frm1702RT:txtPg3Sc1I12Research",
        "frm1702RT:txtPg3Sc1I13Salaries",
        "frm1702RT:txtPg3Sc1I14Contributions",
        "frm1702RT:txtPg3Sc1I15TaxesandLicenses",
        "frm1702RT:txtPg3Sc1I16TransportationandTravel",
        "frm1702RT:txtPg3Sc1I17aJanitorial",
        "frm1702RT:txtPg3Sc1I17bProfessionalFees",
        "frm1702RT:txtPg3Sc1I17cSecurityServices",
    ];
    for (key, value) in keys.into_iter().zip(schedule.source_amounts()) {
        insert_money(fields, key, value);
    }
    for (index, row) in schedule.other.iter().enumerate() {
        let suffix = char::from(b'd' + u8::try_from(index).unwrap_or(0));
        insert(
            fields,
            &format!("frm1702RT:txtPg3Sc1I17{suffix}C1"),
            row.description.clone(),
        );
        insert_money(
            fields,
            &format!("frm1702RT:txtPg3Sc1I17{suffix}C2"),
            row.amount,
        );
    }
    insert_money(
        fields,
        "frm1702RT:txtPg3Sc1I18TotalOrdinaryAllowable",
        schedule.item_18_total,
    );
}

fn insert_schedule_2(fields: &mut BTreeMap<String, String>, schedule: &Form1702RTSchedule2) {
    for (index, row) in schedule.rows.iter().enumerate() {
        let item = index + 1;
        insert(
            fields,
            &format!("frm1702RT:txtPg3Sc2I{item}C1"),
            row.description.clone(),
        );
        insert(
            fields,
            &format!("frm1702RT:txtPg3Sc2I{item}C2"),
            row.legal_basis.clone(),
        );
        insert_money(fields, &format!("frm1702RT:txtPg3Sc2I{item}C3"), row.amount);
    }
    insert_money(
        fields,
        "frm1702RT:txtPg3Sc2I5TotalSpecialAllowable",
        schedule.item_5_total,
    );
}

fn insert_schedule_3(fields: &mut BTreeMap<String, String>, schedule: &Form1702RTSchedule3) {
    insert_money(
        fields,
        "frm1702RT:txtPg4Sc3I1GrossIncome",
        schedule.item_1_gross_income,
    );
    insert_money(
        fields,
        "frm1702RT:txtPg4Sc3I2TotalDeductions",
        schedule.item_2_ordinary_deductions,
    );
    insert_money(
        fields,
        "frm1702RT:txtPg4Sc3I3NetOperatingLoss",
        schedule.item_3_net_operating_loss,
    );
    for (index, row) in schedule.rows.iter().enumerate() {
        let item = index + 4;
        insert(
            fields,
            &format!("frm1702RT:txtPg4Sc3AI{item}C1"),
            row.year_incurred.clone(),
        );
        for (column, value) in [
            (2, row.amount),
            (3, row.applied_previous_years),
            (4, row.expired),
            (5, row.applied_current_year),
            (6, row.unapplied_balance),
        ] {
            insert_money(
                fields,
                &format!("frm1702RT:txtPg4Sc3AI{item}C{column}"),
                value,
            );
        }
    }
    // The reviewed save uses `Sc4` in this otherwise Schedule III key.
    insert_money(
        fields,
        "frm1702RT:txtPg4Sc4I8TotalNOLCO",
        schedule.item_8_total_applied_current_year,
    );
}

fn insert_schedule_4(fields: &mut BTreeMap<String, String>, schedule: &Form1702RTSchedule4) {
    for (index, row) in schedule.rows.iter().enumerate() {
        let item = index + 1;
        insert(
            fields,
            &format!("frm1702RT:txtPg4Sc4I{item}C1"),
            row.year.clone(),
        );
        for (column, value) in [
            (2, row.normal_income_tax),
            (3, row.mcit),
            (4, row.excess_mcit),
            (5, row.applied_previous_years),
            (6, row.expired),
            (7, row.applied_current_year),
            (8, row.allowable_balance),
        ] {
            insert_money(
                fields,
                &format!("frm1702RT:txtPg4Sc4I{item}C{column}"),
                value,
            );
        }
    }
    insert_money(
        fields,
        "frm1702RT:txtPg4Sc4I4TotalExcessMCIT",
        schedule.item_4_total_applied_current_year,
    );
}

fn insert_schedule_5(fields: &mut BTreeMap<String, String>, schedule: &Form1702RTSchedule5) {
    insert_money(
        fields,
        "frm1702RT:txtPg4Sc5I1NetIncome",
        schedule.item_1_net_income_or_loss_per_books,
    );
    for (index, row) in schedule.additions.iter().enumerate() {
        let item = index + 2;
        insert(
            fields,
            &format!("frm1702RT:txtPg4Sc5I{item}C1"),
            row.description.clone(),
        );
        insert_money(fields, &format!("frm1702RT:txtPg4Sc5I{item}C2"), row.amount);
    }
    insert_money(fields, "frm1702RT:txtPg4Sc5I4Total", schedule.item_4_total);
    for (offset, row) in schedule
        .non_taxable_income
        .iter()
        .chain(schedule.special_deductions.iter())
        .enumerate()
    {
        let item = offset + 5;
        insert(
            fields,
            &format!("frm1702RT:txtPg4Sc5I{item}C1"),
            row.description.clone(),
        );
        insert_money(fields, &format!("frm1702RT:txtPg4Sc5I{item}C2"), row.amount);
    }
    insert_money(fields, "frm1702RT:txtPg4Sc5I9Total", schedule.item_9_total);
    insert_money(
        fields,
        "frm1702RT:txtPg4Sc5I10NetTaxableIncome",
        schedule.item_10_net_taxable_income_or_loss,
    );
}

fn insert_transport_fields(fields: &mut BTreeMap<String, String>, draft: &Form1702RTDraft) {
    for key in [
        "frm1702RT:txtPg2Pt4I54CtrModal",
        "frm1702RT:txtPg3Sc1I17iCtrModal",
        "frm1702RT:txtPg3Sc2I4CtrModal",
        "frm1702RT:txtPg4Sc5I3CtrModal",
        "frm1702RT:txtPg4Sc5I6CtrModal",
        "frm1702RT:txtPg4Sc5I8CtrModal",
        "frm1702RT:txtPg4Sc3I3Subtotal",
        "frm1702RT:txtPg4Sc4I4Subtotal",
        "frm1702RT:txtPg3Sc1I17iSubtotal",
        "frm1702RT:txtPg3Sc2I4Subtotal",
        "frm1702RT:txtPg4Sc3AI7C2Subtotal",
        "frm1702RT:txtPg4Sc3AI7C3Subtotal",
        "frm1702RT:txtPg4Sc3AI7C4Subtotal",
        "frm1702RT:txtPg4Sc3AI7C5Subtotal",
        "frm1702RT:txtPg4Sc3AI7C6Subtotal",
        "frm1702RT:txtPg2Pt4I54Subtotal",
        "frm1702RT:txtPg4Sc5I3Subtotal",
        "frm1702RT:txtPg4Sc5I6Subtotal",
        "frm1702RT:txtPg4Sc5I8Subtotal",
    ] {
        let value = draft
            .preserved_transport_fields
            .get(key)
            .cloned()
            .unwrap_or_else(|| "0".to_string());
        insert(fields, key, value);
    }
    insert(fields, "frm1702RT:txtCurrentPage", "1");
    insert(fields, "frm1702RT:txtMaxPage", "4");
    insert(fields, "driveSelectTPExport", "0");
    insert(fields, "txtFinalFlag", draft.xml_final_flag.clone());
    insert(fields, "txtEnroll", "Y");
    insert(fields, "ebirOnlineConfirmUsername", "");
    insert(fields, "ebirOnlineUsername", "");
    insert(fields, "ebirOnlineSecret", "");
}

impl Form1702RTDraft {
    pub fn from_bir_field_map(
        fields: &BTreeMap<String, String>,
    ) -> Result<Self, Vec<(String, String)>> {
        let mut errors = Vec::new();
        let expected = expected_xml_keys();
        if expected.len() != EXACT_SOURCE_FIELD_COUNT {
            errors.push((
                "xml_contract".to_string(),
                format!(
                    "Internal 1702RT contract has {} fields instead of {}",
                    expected.len(),
                    EXACT_SOURCE_FIELD_COUNT
                ),
            ));
        }
        let actual = fields.keys().cloned().collect::<BTreeSet<_>>();
        for missing in expected.difference(&actual) {
            errors.push((
                missing.clone(),
                "Required 1702RT source field is missing".to_string(),
            ));
        }
        for unexpected in actual.difference(&expected) {
            errors.push((
                unexpected.clone(),
                "Field is not part of the reviewed 1702RTv2018C contract".to_string(),
            ));
        }
        for (key, value) in [
            ("frm1702RT:txtCurrentPage", "1"),
            ("frm1702RT:txtMaxPage", "4"),
            ("driveSelectTPExport", "0"),
            ("txtEnroll", "Y"),
            ("ebirOnlineConfirmUsername", ""),
            ("ebirOnlineUsername", ""),
            ("ebirOnlineSecret", ""),
        ] {
            require_exact(fields, key, value, &mut errors);
        }
        if !matches!(field(fields, "txtFinalFlag"), "0" | "1") {
            errors.push((
                "txtFinalFlag".to_string(),
                "Expected reviewed value 0 or 1".to_string(),
            ));
        }

        let filing_basis = parse_exclusive_pair(
            fields,
            "frm1702RT:rdoPg1I1Calendar",
            "frm1702RT:rdoPg1I1Fiscal",
            "filing_basis",
            &mut errors,
        )
        .map(|first| {
            if first {
                Form1702RTFilingBasis::Calendar
            } else {
                Form1702RTFilingBasis::Fiscal
            }
        })
        .unwrap_or_default();
        let is_amended = parse_exclusive_pair(
            fields,
            "frm1702RT:rdoPg1I3AmmendYes",
            "frm1702RT:rdoPg1I3AmmendNo",
            "is_amended",
            &mut errors,
        )
        .unwrap_or(false);
        let is_short_period = parse_exclusive_pair(
            fields,
            "frm1702RT:rdoPg1I4ShortPeriodYes",
            "frm1702RT:rdoPg1I4ShortPeriodNo",
            "is_short_period",
            &mut errors,
        )
        .unwrap_or(false);
        let itemized = parse_bool(
            fields,
            "frm1702RT:rdoPg1Pt1I13ItemizedDeduction",
            &mut errors,
        );
        let osd = parse_bool(
            fields,
            "frm1702RT:rdoPg1Pt1I13OptionalStandard",
            &mut errors,
        );
        let deduction_method = match (itemized, osd) {
            (Some(true), Some(false)) => Form1702RTDeductionMethod::Itemized,
            (Some(false), Some(true)) => Form1702RTDeductionMethod::OptionalStandard,
            (Some(false), Some(false)) => Form1702RTDeductionMethod::Unresolved,
            (Some(true), Some(true)) => {
                errors.push((
                    "deduction_method".to_string(),
                    "Item 13 choices cannot both be selected".to_string(),
                ));
                Form1702RTDeductionMethod::Unresolved
            }
            _ => Form1702RTDeductionMethod::Unresolved,
        };

        let tin_segments = [
            field(fields, "frm1702RT:txtPg1Pt1I6TIN1"),
            field(fields, "frm1702RT:txtPg1Pt1I6TIN2"),
            field(fields, "frm1702RT:txtPg1Pt1I6TIN3"),
            field(fields, "frm1702RT:txtPg1Pt1I6TIN4"),
        ];
        let tin = tin_segments.concat();
        for page in 2..=4 {
            for (segment, source) in (1..=4).zip(tin_segments) {
                verify_equal(
                    fields,
                    &format!("frm1702RT:txtPg{page}TIN{segment}"),
                    source,
                    &mut errors,
                );
            }
            verify_equal(
                fields,
                &format!("txtBranchMaskP{page}"),
                tin_segments[3],
                &mut errors,
            );
        }
        verify_equal(fields, "BranchMaskP1", tin_segments[3], &mut errors);
        verify_equal(
            fields,
            "frm1702RT:drpPg1Pt1I7RDOCode",
            field(fields, "frm1702RT:txtRDO"),
            &mut errors,
        );

        let registered_name_lines = std::array::from_fn(|index| {
            field(fields, &format!("frm1702RT:txtPg1Pt1I8Name{}", index + 1)).to_string()
        });
        let registered_address_lines = std::array::from_fn(|index| {
            field(
                fields,
                &format!("frm1702RT:txtPg1Pt1I9Address{}", index + 1),
            )
            .to_string()
        });
        let taxpayer_name = field(fields, "frm1702RT:txtPg2RegisteredName").to_string();
        for page in 3..=4 {
            verify_equal(
                fields,
                &format!("frm1702RT:txtPg{page}RegisteredName"),
                &taxpayer_name,
                &mut errors,
            );
        }

        let overpayment_flags = [
            parse_bool(
                fields,
                "frm1702RT:rdoPg1Pt2I21OverpaymentRefunded",
                &mut errors,
            ),
            parse_bool(
                fields,
                "frm1702RT:rdoPg1Pt2I21OverpaymentIssued",
                &mut errors,
            ),
            parse_bool(
                fields,
                "frm1702RT:rdoPg1Pt2I21OverpaymentCarried",
                &mut errors,
            ),
        ];
        let selected = overpayment_flags
            .iter()
            .filter(|flag| **flag == Some(true))
            .count();
        if selected > 1 {
            errors.push((
                "part_ii.overpayment_disposition".to_string(),
                "Only one overpayment disposition may be selected".to_string(),
            ));
        }
        let overpayment_disposition = if overpayment_flags[0] == Some(true) {
            Some(Form1702RTOverpaymentDisposition::Refund)
        } else if overpayment_flags[1] == Some(true) {
            Some(Form1702RTOverpaymentDisposition::TaxCreditCertificate)
        } else if overpayment_flags[2] == Some(true) {
            Some(Form1702RTOverpaymentDisposition::CarryOver)
        } else {
            None
        };

        let mut draft = Form1702RTDraft {
            penalty: crate::forms::penalty_items::PenaltyState::imported(),
            id: None,
            tin,
            taxable_year: parse_taxable_year(fields, "frm1702RT:txtPg1I2Year", &mut errors),
            month: parse_u8(fields, "frm1702RT:ddlPg1I2Month", &mut errors),
            filing_basis,
            is_amended,
            is_short_period,
            atc: Form1702RTAtcSelection {
                printed_mcit_selected: parse_bool(fields, "frm1702RT:rdoPg1I5Atc", &mut errors)
                    .unwrap_or(false),
                other_selected: parse_bool(fields, "frm1702RT:rdoPg1I5AtcOther", &mut errors)
                    .unwrap_or(false),
                other_code: field(fields, "frm1702RT:drpPg1I5AtcOther").to_string(),
            },
            rdo_code: field(fields, "frm1702RT:txtRDO").to_string(),
            taxpayer_name,
            registered_name_lines,
            registered_address: registered_address_lines
                .iter()
                .filter(|value| !value.trim().is_empty())
                .cloned()
                .collect::<Vec<_>>()
                .join(" "),
            registered_address_lines,
            zip_code: field(fields, "frm1702RT:txtZIP").to_string(),
            incorporation_date: parse_optional_date(fields, "frm1702RT:txtPg1Pt1I10", &mut errors),
            contact_number: field(fields, "frm1702RT:txtPg1Pt1I11Contact").to_string(),
            email: field(fields, "frm1702RT:txtPg1Pt1I12Email").to_string(),
            deduction_method,
            part_ii: parse_part_ii(fields, overpayment_disposition, &mut errors),
            payment_details: parse_payment_rows(fields, &mut errors),
            part_iv: parse_part_iv(fields, &mut errors),
            part_v: parse_part_v(fields, &mut errors),
            schedule_1: parse_schedule_1(fields, &mut errors),
            schedule_2: parse_schedule_2(fields, &mut errors),
            schedule_3: parse_schedule_3(fields, &mut errors),
            schedule_4: parse_schedule_4(fields, &mut errors),
            schedule_5: parse_schedule_5(fields, &mut errors),
            more_rows: Default::default(),
            president_signature: field(fields, "frm1702RT:txtSignaturePresident").to_string(),
            treasurer_signature: field(fields, "frm1702RT:txtSignatureTreasurer").to_string(),
            president_signatory_title: field(fields, "frm1702RT:txtPg1Pt2Signatory1").to_string(),
            president_signatory_tin: field(fields, "frm1702RT:txtPg1Pt2SignatoryTin1").to_string(),
            treasurer_signatory_title: field(fields, "frm1702RT:txtPg1Pt2Signatory2").to_string(),
            treasurer_signatory_tin: field(fields, "frm1702RT:txtPg1Pt2SignatoryTin2").to_string(),
            number_of_attachments: field(fields, "frm1702RT:txtPg1Pt2PagesFilled").to_string(),
            xml_final_flag: field(fields, "txtFinalFlag").to_string(),
            preserved_transport_fields: fields.clone(),
            calculation_issues: Vec::new(),
            last_error: None,
            lifecycle: super::queueable::SubmissionLifecycle::default(),
        };
        // An imported return carries the penalties it was prepared with:
        // keep each one as an override so the source amounts round-trip.
        Form1702RTDraft::penalty_declaration().adopt_imported(&mut draft);
        if errors.is_empty() {
            Ok(draft)
        } else {
            Err(errors)
        }
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    fn contract_draft() -> Form1702RTDraft {
        let mut draft = Form1702RTDraft {
            tin: "00000000000000".to_string(),
            taxable_year: 2025,
            month: 12,
            filing_basis: Form1702RTFilingBasis::Calendar,
            atc: Form1702RTAtcSelection {
                printed_mcit_selected: true,
                other_selected: true,
                other_code: "IC010".to_string(),
            },
            rdo_code: "018".to_string(),
            taxpayer_name: "JUAN DELA CRUZ".to_string(),
            registered_name_lines: ["JUAN DELA CRUZ".to_string(), String::new(), String::new()],
            registered_address: "OLONGAPO".to_string(),
            registered_address_lines: ["OLONGAPO".to_string(), String::new(), String::new()],
            zip_code: "2200".to_string(),
            incorporation_date: Form1702RTDate::new(2019, 12, 10).ok(),
            contact_number: "09123456789".to_string(),
            email: "TAXPAYER@EXAMPLE.COM".to_string(),
            deduction_method: Form1702RTDeductionMethod::OptionalStandard,
            president_signatory_title: "PRESIDENT".to_string(),
            president_signatory_tin: "12345678900000".to_string(),
            treasurer_signatory_title: "TREASURER".to_string(),
            treasurer_signatory_tin: "98765432100000".to_string(),
            number_of_attachments: "000".to_string(),
            xml_final_flag: "1".to_string(),
            ..Form1702RTDraft::default()
        };
        draft.part_iv.item_27_sales = WholePeso(1_000);
        draft.part_iv.item_28_sales_returns = WholePeso(1_000);
        draft.part_iv.item_30_cost_of_sales_or_services = WholePeso(1_000);
        draft.part_iv.item_32_other_taxable_income = WholePeso(1_000);
        draft.part_iv.item_40_income_tax_rate_percent = 30;
        draft.part_iv.tax_credits.item_44_prior_year_excess_credits = WholePeso(1_000);
        draft.part_ii.item_17_surcharge = WholePeso(1_000);
        draft.part_ii.item_18_interest = WholePeso(1_000);
        draft.part_ii.item_19_compromise = WholePeso(1_000);
        crate::forms::queueable::QueueableForm::keep_entered_penalties(&mut draft);
        draft.recompute();
        draft
    }

    #[test]
    fn exact_contract_has_258_fields_and_four_pages() {
        let fields = contract_draft().to_bir_field_map();
        assert_eq!(fields.len(), EXACT_SOURCE_FIELD_COUNT);
        assert_eq!(expected_xml_keys().len(), EXACT_SOURCE_FIELD_COUNT);
        assert_eq!(fields["frm1702RT:txtMaxPage"], "4");
    }

    #[test]
    fn checked_import_round_trips_negative_whole_pesos_losslessly() {
        let mut source = contract_draft().to_bir_field_map();
        source.insert(
            "frm1702RT:txtPg1Pt2I16NetTax".to_string(),
            "-8,000".to_string(),
        );
        source.insert(
            "frm1702RT:txtPg2Pt4I56NetTax".to_string(),
            "-8,000".to_string(),
        );
        let imported = Form1702RTDraft::from_bir_field_map(&source).expect("source shape imports");
        assert_eq!(
            imported.part_ii.item_16_net_tax_payable_or_overpayment,
            WholePeso(-8_000)
        );
        assert_eq!(imported.to_bir_field_map(), source);
    }

    #[test]
    fn malformed_money_is_rejected_instead_of_becoming_zero() {
        let mut source = contract_draft().to_bir_field_map();
        source.insert("frm1702RT:txtPg2Pt4I27Sales".to_string(), "1,2".to_string());
        let errors = Form1702RTDraft::from_bir_field_map(&source).expect_err("must reject");
        assert!(
            errors
                .iter()
                .any(|(field, _)| field == "frm1702RT:txtPg2Pt4I27Sales")
        );
    }

    #[test]
    fn generated_pseudo_xml_round_trips_through_checked_parser() {
        let draft = contract_draft();
        let xml = draft.to_bir_xml_payload();
        let imported = Form1702RTDraft::from_bir_xml_payload(&xml).expect("generated XML imports");
        assert_eq!(imported.president_signatory_title, "PRESIDENT");
        assert_eq!(imported.treasurer_signatory_title, "TREASURER");
        assert!(xml.contains("frm1702RT:txtPg1Pt2Signatory1=PRESIDENT"));
        assert!(xml.contains("frm1702RT:txtPg1Pt2Signatory2=TREASURER"));
        assert_eq!(imported.to_bir_field_map(), draft.to_bir_field_map());
    }

    #[test]
    fn imported_penalties_are_kept_as_overrides() {
        // The return was prepared for a period now long past its deadline:
        // the imported penalty items are kept as overrides, so recomputing
        // today files exactly what the source carried.
        let mut draft = Form1702RTDraft::from_bir_field_map(&contract_draft().to_bir_field_map())
            .expect("source shape imports");
        let fields = draft.to_bir_field_map();
        let items = Form1702RTDraft::penalty_declaration();
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
