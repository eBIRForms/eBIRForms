//! Checked 160-field editable-save contract for exact form `2550Qv2024`.
//!
//! The reviewed plain and decrypted encrypted payloads have the same field
//! union. Their only semantic transport difference is `txtFinalFlag` (`1` in
//! the editable save and `0` in the encrypted companion); the encrypted file
//! also writes `dateFiled` as a standalone element rather than a field div.

use std::collections::{BTreeMap, BTreeSet};

use bir_rules::{RawValue, StableInstanceId};

use crate::form_rules::{
    form_2550q_group_bindings, form_2550q_singleton_field_ids,
    seed_raw_editor_state_from_reviewed_fields,
};

use super::FormValidator;
use super::form_2550q::{
    Form2550QAdvanceVatRow, Form2550QCapitalGoodRow, Form2550QCreditableVatRow, Form2550QDate,
    Form2550QDraft, Form2550QFilingBasis, Form2550QLocalPrintFields, Form2550QPartII,
    Form2550QPartIV, Form2550QQuarter, Form2550QRowFamily, Form2550QSchedule2,
    Form2550QTaxpayerClassification, Form2550QXmlFinalFlag, PRESERVED_UNMODELED_XML_KEYS,
    is_preserved_unmodeled_xml_key,
};

const EXACT_SOURCE_FIELD_COUNT: usize = 160;
const SCHEDULE_ROW_SUFFIXES: [u8; 2] = [10, 11];
const SCHEDULE_3_SUFFIXES: [u8; 2] = [0, 1];
const SCHEDULE_4_SUFFIXES: [u8; 2] = [0, 1];
const REQUIRED_RAW_AUTHORITY_KEYS: [&str; 26] = [
    "frm2550qv2024:txtYearNo2",
    "frm2550qv2024:calendarNo1",
    "frm2550qv2024:fiscalNo1",
    "frm2550qv2024:OptQuarter1",
    "frm2550qv2024:OptQuarter2",
    "frm2550qv2024:OptQuarter3",
    "frm2550qv2024:OptQuarter4",
    "frm2550qv2024:taxpayerAddress",
    "frm2550qv2024:taxpayerZip",
    "frm2550qv2024:taxpayerContactNumber",
    "frm2550qv2024:taxpayerEmailAddress",
    "frm2550qv2024:taxPayerClassification1",
    "frm2550qv2024:taxPayerClassification2",
    "frm2550qv2024:taxPayerClassification3",
    "frm2550qv2024:taxPayerClassification4",
    "frm2550qv2024:internationalTreatyYn",
    "frm2550qv2024:specialRateYn",
    "frm2550qv2024:specifyInternationalTreaty",
    "frm2550qv2024:addSpecifyNo19",
    "frm2550qv2024:otherCreditsNo19",
    "frm2550qv2024:addSpecifyNo42",
    "frm2550qv2024:otherSpecify42",
    "frm2550qv2024:addSpecifyNo47",
    "frm2550qv2024:otherSpecify47",
    "frm2550qv2024:addSpecifyNo56",
    "frm2550qv2024:otherSpecify56",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReviewedRawValueKind {
    Boolean,
    Text,
    Money,
    Date,
    RequiredU8,
    RequiredU16,
    OptionalU16,
    ReceiptIdentifier,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ReviewedRawLocation {
    Singleton(&'static str),
    Repeated {
        family: Form2550QRowFamily,
        instance_id: StableInstanceId,
        member_key: &'static str,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReviewedRawBindingTarget {
    stable_path: String,
    xml_key: String,
    kind: ReviewedRawValueKind,
    location: ReviewedRawLocation,
}

impl ReviewedRawBindingTarget {
    fn raw_value<'draft>(&self, draft: &'draft Form2550QDraft) -> Option<&'draft RawValue> {
        match &self.location {
            ReviewedRawLocation::Singleton(field_key) => {
                draft.raw_editor_state.singleton_value(field_key)
            }
            ReviewedRawLocation::Repeated {
                family,
                instance_id,
                member_key,
            } => draft
                .raw_editor_state
                .repeated_value(*family, instance_id, member_key),
        }
    }
}

impl Form2550QDraft {
    /// Raw authority that checked XML requires but the draft does not carry.
    ///
    /// This is a *checked-XML export* requirement, not a general draft
    /// invariant. Raw values are only ever seeded from a checked XML import, so
    /// a purely typed draft has none - and for it the typed fields are the
    /// authority. Running this as part of `validate()` would make every typed
    /// draft (print preview, render-contract fixtures) report a wall of
    /// missing-value errors that no user action can clear. The refusal to
    /// synthesize still binds, fully, wherever XML is actually produced.
    pub(super) fn missing_required_raw_authority_errors(&self) -> Vec<(String, String)> {
        let mut errors = Vec::new();
        for target in self.reviewed_raw_binding_targets() {
            if target.raw_value(self).is_none()
                && REQUIRED_RAW_AUTHORITY_KEYS.contains(&target.xml_key.as_str())
            {
                errors.push((
                    format!("raw_editor.{}", target.stable_path),
                    "Required raw value is missing; checked XML refuses to synthesize \
                     it from the typed draft"
                        .to_string(),
                ));
            }
        }
        errors
    }

    pub(super) fn raw_editor_coherence_errors(&self) -> Vec<(String, String)> {
        let typed_fields = self.to_reviewed_field_map();
        let mut errors = Vec::new();
        for target in self.reviewed_raw_binding_targets() {
            let Some(raw_value) = target.raw_value(self) else {
                continue;
            };
            if self
                .raw_editor_state
                .malformed_fields()
                .contains_key(&target.stable_path)
            {
                continue;
            }
            let expected = typed_fields
                .get(&target.xml_key)
                .expect("reviewed raw binding targets only exact 2550Q XML keys");
            match raw_matches_reviewed_value(raw_value, expected, target.kind) {
                Ok(true) => {}
                Ok(false) => errors.push((
                    format!("raw_editor.{}", target.stable_path),
                    "Captured raw editor text disagrees with the typed draft value; checked XML \
                     refuses the typed projection and leaves the raw text unchanged"
                        .to_string(),
                )),
                Err(()) => errors.push((
                    format!("raw_editor.{}", target.stable_path),
                    "Captured raw editor text is malformed even though no malformed marker was \
                     persisted; checked XML refuses the typed projection and leaves the raw text \
                     unchanged"
                        .to_string(),
                )),
            }
        }
        errors
    }

    fn reviewed_raw_binding_targets(&self) -> Vec<ReviewedRawBindingTarget> {
        let mut targets = form_2550q_singleton_field_ids()
            .iter()
            .copied()
            .map(|field_key| ReviewedRawBindingTarget {
                stable_path: field_key.to_string(),
                xml_key: field_key.to_string(),
                kind: singleton_raw_value_kind(field_key),
                location: ReviewedRawLocation::Singleton(field_key),
            })
            .collect::<Vec<_>>();

        for binding in form_2550q_group_bindings() {
            let (instance_ids, suffixes): (Vec<Option<StableInstanceId>>, &[u8]) =
                match binding.family() {
                    Form2550QRowFamily::Schedule1 => (
                        self.schedule_1
                            .iter()
                            .map(|row| row.instance_id.clone())
                            .collect(),
                        &SCHEDULE_ROW_SUFFIXES,
                    ),
                    Form2550QRowFamily::Schedule3 => (
                        self.schedule_3
                            .iter()
                            .map(|row| row.instance_id.clone())
                            .collect(),
                        &SCHEDULE_3_SUFFIXES,
                    ),
                    Form2550QRowFamily::Schedule4 => (
                        self.schedule_4
                            .iter()
                            .map(|row| row.instance_id.clone())
                            .collect(),
                        &SCHEDULE_4_SUFFIXES,
                    ),
                    Form2550QRowFamily::Item19Additional
                    | Form2550QRowFamily::Item42Additional
                    | Form2550QRowFamily::Item47Additional
                    | Form2550QRowFamily::Item56Additional => continue,
                };
            for (instance_id, suffix) in instance_ids.into_iter().zip(suffixes.iter().copied()) {
                let Some(instance_id) = instance_id else {
                    continue;
                };
                for member in binding.members() {
                    let Some(xml_prefix) = member.reviewed_xml_prefix() else {
                        continue;
                    };
                    let member_key = member.field_id();
                    targets.push(ReviewedRawBindingTarget {
                        stable_path: format!(
                            "{}/{instance_id}/{member_key}",
                            raw_family_path(binding.family())
                        ),
                        xml_key: format!("{xml_prefix}{suffix}"),
                        kind: repeated_raw_value_kind(binding.family(), member_key),
                        location: ReviewedRawLocation::Repeated {
                            family: binding.family(),
                            instance_id: instance_id.clone(),
                            member_key,
                        },
                    });
                }
            }
        }
        targets
    }

    /// Serialize the exact reviewed editable-save key set. A clone is
    /// recomputed first so stale JSON-derived totals never enter the payload.
    /// Print preview uses this map. XML emission still goes through
    /// [`Self::try_to_bir_xml_payload`], which also gates raw buffers.
    pub fn to_bir_field_map(&self) -> BTreeMap<String, String> {
        let mut normalized = self.clone();
        normalized.recompute();
        normalized.to_reviewed_field_map()
    }

    fn to_reviewed_field_map(&self) -> BTreeMap<String, String> {
        let expected = expected_xml_keys();
        assert_eq!(expected.len(), EXACT_SOURCE_FIELD_COUNT);
        let mut fields = BTreeMap::new();
        for key in PRESERVED_UNMODELED_XML_KEYS {
            if let Some(value) = self.preserved_unmodeled_xml_fields.get(key) {
                fields.insert(key.to_string(), value.clone());
            }
        }
        let (tin1, tin2, tin3, branch) = split_tin(&self.tin);

        insert_bool_pair(
            &mut fields,
            "frm2550qv2024:calendarNo1",
            "frm2550qv2024:fiscalNo1",
            matches!(self.filing_basis, Form2550QFilingBasis::Calendar),
        );
        insert(
            &mut fields,
            "frm2550qv2024:selectedMonthNo2",
            format!("{:02}", self.year_end_month),
        );
        insert(
            &mut fields,
            "frm2550qv2024:txtYearNo2",
            self.taxable_year.to_string(),
        );
        insert_one_of_four(
            &mut fields,
            "frm2550qv2024:OptQuarter",
            self.quarter.number(),
        );
        insert_optional_date(
            &mut fields,
            "frm2550qv2024:RtnPeriodFromNo4",
            self.return_period_from,
        );
        insert_optional_date(
            &mut fields,
            "frm2550qv2024:RtnPeriodToNo4",
            self.return_period_to,
        );
        insert_bool_pair(
            &mut fields,
            "frm2550qv2024:amendedReturnYesNo5",
            "frm2550qv2024:amendedReturnNo5",
            self.is_amended,
        );
        insert_bool_pair(
            &mut fields,
            "frm2550qv2024:OptShortPrd1",
            "frm2550qv2024:OptShortPrd2",
            self.is_short_period_return,
        );

        insert(&mut fields, "frm2550qv2024:txtTIN1", tin1.clone());
        insert(&mut fields, "frm2550qv2024:txtTIN2", tin2.clone());
        insert(&mut fields, "frm2550qv2024:txtTIN3", tin3.clone());
        insert(&mut fields, "frm2550qv2024:branchCode", branch.clone());
        insert(
            &mut fields,
            "frm2550qv2024:txtRDOCode",
            self.rdo_code.clone(),
        );
        insert(
            &mut fields,
            "frm2550qv2024:taxpayerName",
            self.taxpayer_name.clone(),
        );
        insert(
            &mut fields,
            "frm2550qv2024:taxpayerAddress",
            self.registered_address.clone(),
        );
        insert(
            &mut fields,
            "frm2550qv2024:taxpayerZip",
            self.zip_code.clone(),
        );
        insert(
            &mut fields,
            "frm2550qv2024:taxpayerContactNumber",
            self.contact_number.clone(),
        );
        insert(
            &mut fields,
            "frm2550qv2024:taxpayerEmailAddress",
            self.email.clone(),
        );
        insert_taxpayer_classification(&mut fields, self.taxpayer_classification);
        insert_bool_pair(
            &mut fields,
            "frm2550qv2024:internationalTreatyYn",
            "frm2550qv2024:specialRateYn",
            self.is_availing_tax_relief,
        );
        insert(
            &mut fields,
            "frm2550qv2024:specifyInternationalTreaty",
            self.tax_relief_details.clone(),
        );

        let p2 = &self.part_ii;
        insert_money(
            &mut fields,
            "frm2550qv2024:excessInputTax",
            p2.item_15_net_vat_payable_or_excess,
        );
        insert_money(
            &mut fields,
            "frm2550qv2024:creditableVat",
            p2.item_16_creditable_vat_withheld,
        );
        insert_money(
            &mut fields,
            "frm2550qv2024:advVatPayment",
            p2.item_17_advance_vat_payments,
        );
        insert_money(
            &mut fields,
            "frm2550qv2024:vatPaidReturn",
            p2.item_18_paid_on_previous_return,
        );
        insert(
            &mut fields,
            "frm2550qv2024:addSpecifyNo19",
            p2.item_19_description.clone(),
        );
        insert_money(
            &mut fields,
            "frm2550qv2024:otherCreditsNo19",
            p2.item_19_other_credit_or_payment,
        );
        insert_money(
            &mut fields,
            "frm2550qv2024:totalTaxCredits",
            p2.item_20_total_credits_or_payments,
        );
        insert_money(
            &mut fields,
            "frm2550qv2024:excessCredits",
            p2.item_21_tax_payable_or_excess_credits,
        );
        insert_money(&mut fields, "frm2550qv2024:surcharge", p2.item_22_surcharge);
        insert_money(&mut fields, "frm2550qv2024:interest", p2.item_23_interest);
        insert_money(
            &mut fields,
            "frm2550qv2024:compromise",
            p2.item_24_compromise,
        );
        insert_money(
            &mut fields,
            "frm2550qv2024:penalties",
            p2.item_25_total_penalties,
        );
        insert_money(
            &mut fields,
            "frm2550qv2024:totalPayable",
            p2.item_26_total_amount_payable_or_excess,
        );

        insert(&mut fields, "frm2550qv2024:txtPg2TIN1", tin1);
        insert(&mut fields, "frm2550qv2024:txtPg2TIN2", tin2);
        insert(&mut fields, "frm2550qv2024:txtPg2TIN3", tin3);
        insert(&mut fields, "frm2550qv2024:txtPg2BranchCode", branch);
        insert(
            &mut fields,
            "frm2550qv2024:Pg2TaxPayer",
            self.taxpayer_name.clone(),
        );

        let p4 = &self.part_iv;
        for (key, value) in [
            ("frm2550qv2024:vatableSales", p4.item_31a_vatable_sales),
            ("frm2550qv2024:outputVatSales", p4.item_31b_output_tax),
            ("frm2550qv2024:zeroRatedSales", p4.item_32a_zero_rated_sales),
            ("frm2550qv2024:exemptSales", p4.item_33a_exempt_sales),
            ("frm2550qv2024:totalSales", p4.item_34a_total_sales),
            ("frm2550qv2024:outputTaxDue", p4.item_34b_output_tax_due),
            (
                "frm2550qv2024:lessOutputVat",
                p4.item_35b_less_output_vat_uncollected,
            ),
            (
                "frm2550qv2024:addOutputVat",
                p4.item_36b_add_output_vat_recovered,
            ),
            (
                "frm2550qv2024:totalAdjOutput",
                p4.item_37b_adjusted_output_tax_due,
            ),
            (
                "frm2550qv2024:inputTaxCarried",
                p4.item_38b_input_tax_carried,
            ),
            (
                "frm2550qv2024:inputTaxDeferred",
                p4.item_39b_input_tax_deferred,
            ),
            (
                "frm2550qv2024:transitionalInputTax",
                p4.item_40b_transitional_input_tax,
            ),
            (
                "frm2550qv2024:presumptiveInputTax",
                p4.item_41b_presumptive_input_tax,
            ),
            ("frm2550qv2024:otherSpecify42", p4.item_42b_other_input_tax),
            ("frm2550qv2024:total43", p4.item_43b_total_prior_input_tax),
            (
                "frm2550qv2024:domesticPurchase",
                p4.item_44a_domestic_purchases,
            ),
            (
                "frm2550qv2024:domesticInputTax",
                p4.item_44b_domestic_input_tax,
            ),
            (
                "frm2550qv2024:servicesPurchase",
                p4.item_45a_nonresident_services,
            ),
            (
                "frm2550qv2024:serviceInputTax",
                p4.item_45b_nonresident_service_input_tax,
            ),
            ("frm2550qv2024:importPurchase", p4.item_46a_importations),
            ("frm2550qv2024:importInputTax", p4.item_46b_import_input_tax),
            ("frm2550qv2024:otherSpecify47", p4.item_47a_other_purchases),
            ("otherSpecify47B", p4.item_47b_other_input_tax),
            (
                "frm2550qv2024:domesticPurchaseNoTax",
                p4.item_48a_domestic_purchases_no_input_tax,
            ),
            (
                "frm2550qv2024:vatExemptImports",
                p4.item_49a_vat_exempt_importations,
            ),
            (
                "frm2550qv2024:totalCurPurchase",
                p4.item_50a_total_current_purchases,
            ),
            (
                "frm2550qv2024:totalCurInputTax",
                p4.item_50b_total_current_input_tax,
            ),
            (
                "frm2550qv2024:totalAvailInputTax",
                p4.item_51b_total_available_input_tax,
            ),
            (
                "frm2550qv2024:importCapitalInputTax",
                p4.item_52b_deferred_capital_goods_input_tax,
            ),
            (
                "frm2550qv2024:inputTaxAttr",
                p4.item_53b_input_tax_attributable_to_exempt_sales,
            ),
            (
                "frm2550qv2024:vatRefund",
                p4.item_54b_vat_refund_or_tcc_claimed,
            ),
            (
                "frm2550qv2024:inputVatUnpaid",
                p4.item_55b_input_vat_on_unpaid_payables,
            ),
            ("frm2550qv2024:otherSpecify56", p4.item_56b_other_deduction),
            (
                "frm2550qv2024:totalDeductions",
                p4.item_57b_total_deductions,
            ),
            (
                "frm2550qv2024:addInputVat",
                p4.item_58b_input_vat_on_settled_payables,
            ),
            (
                "frm2550qv2024:adjDeductions",
                p4.item_59b_adjusted_deductions,
            ),
            (
                "frm2550qv2024:totalAllowInputTax",
                p4.item_60b_total_allowable_input_tax,
            ),
            (
                "frm2550qv2024:netVatPayable",
                p4.item_61b_net_vat_payable_or_excess,
            ),
        ] {
            insert_money(&mut fields, key, value);
        }
        for (key, value) in [
            (
                "frm2550qv2024:addSpecifyNo42",
                p4.item_42_description.as_str(),
            ),
            (
                "frm2550qv2024:addSpecifyNo47",
                p4.item_47_description.as_str(),
            ),
            (
                "frm2550qv2024:addSpecifyNo56",
                p4.item_56_description.as_str(),
            ),
        ] {
            insert(&mut fields, key, value);
        }

        for (offset, suffix) in SCHEDULE_ROW_SUFFIXES.into_iter().enumerate() {
            let row = self.schedule_1.get(offset).cloned().unwrap_or_default();
            insert_optional_date(
                &mut fields,
                &format!("txtDatePurchase{suffix}"),
                row.purchase_or_import_date,
            );
            insert(
                &mut fields,
                &format!("txtSourceCode{suffix}"),
                row.source_code,
            );
            insert(
                &mut fields,
                &format!("txtDescription{suffix}"),
                row.description,
            );
            insert_money(
                &mut fields,
                &format!("txtAmountPurchase{suffix}"),
                row.purchase_or_import_amount,
            );
            insert_money(&mut fields, &format!("txtInputTax{suffix}"), row.input_tax);
            insert_life(
                &mut fields,
                &format!("txtEstimatedLife{suffix}"),
                row.estimated_life_months,
            );
            insert_life(
                &mut fields,
                &format!("txtRecognizedLife{suffix}"),
                row.recognized_life_months,
            );
            insert_money(
                &mut fields,
                &format!("txtAllowedInputTax{suffix}"),
                row.allowable_input_tax_for_period,
            );
            insert_money(
                &mut fields,
                &format!("txtBalanceInputTax{suffix}"),
                row.balance_to_next_period,
            );
        }
        insert_money(
            &mut fields,
            "sched1TotalBalPrev",
            self.schedule_1_previous_total(),
        );
        insert_money(
            &mut fields,
            "sched1TotalBalNext",
            self.schedule_1_next_total(),
        );

        let s2 = &self.schedule_2;
        for (key, value) in [
            (
                "frm2550qv2024:sched2InputTaxDirect",
                s2.input_tax_directly_attributable_to_exempt_sales,
            ),
            ("frm2550qv2024:sched2VatExemptSale", s2.vat_exempt_sales),
            (
                "frm2550qv2024:sched2AmountInputTax",
                s2.input_tax_not_directly_attributable,
            ),
            ("frm2550qv2024:sched2TotalSales", s2.total_sales),
            ("frm2550qv2024:sched2TotalRatable", s2.ratable_input_tax),
            (
                "frm2550qv2024:sched2TotalAttr",
                s2.total_input_tax_attributable_to_exempt_sales,
            ),
        ] {
            insert_money(&mut fields, key, value);
        }

        for (offset, suffix) in SCHEDULE_3_SUFFIXES.into_iter().enumerate() {
            let row = self.schedule_3.get(offset).cloned().unwrap_or_default();
            insert_optional_date(
                &mut fields,
                &format!("txtDateCovered3{suffix}"),
                row.period_from,
            );
            insert_optional_date(
                &mut fields,
                &format!("txtDateCovered3To{suffix}"),
                row.period_to,
            );
            insert(
                &mut fields,
                &format!("txtNameWithHoldingAgent3{suffix}"),
                row.withholding_agent_name,
            );
            insert_money(
                &mut fields,
                &format!("txtIncomePayment3{suffix}"),
                row.income_payment,
            );
            insert_money(
                &mut fields,
                &format!("txtTotalTaxWithHeld3{suffix}"),
                row.tax_withheld,
            );
        }
        insert_money(
            &mut fields,
            "sched3TotalIncome",
            self.schedule_3_income_total(),
        );
        insert_money(&mut fields, "sched3TotalTax", self.schedule_3_tax_total());

        for (offset, suffix) in SCHEDULE_4_SUFFIXES.into_iter().enumerate() {
            let row = self.schedule_4.get(offset).cloned().unwrap_or_default();
            insert_optional_date(&mut fields, &format!("txtDate4{suffix}"), row.period_from);
            insert_optional_date(&mut fields, &format!("txtDate4To{suffix}"), row.period_to);
            insert(
                &mut fields,
                &format!("txtNameOfMiller4{suffix}"),
                row.miller_name,
            );
            insert(
                &mut fields,
                &format!("txtNameOfTaxpayer4{suffix}"),
                row.taxpayer_name,
            );
            insert(
                &mut fields,
                &format!("txtOfficialReceiptNumber4{suffix}"),
                if row.official_receipt_number.trim().is_empty() {
                    "0.00".to_string()
                } else {
                    row.official_receipt_number
                },
            );
            insert_money(
                &mut fields,
                &format!("txtAmountPaid4{suffix}"),
                row.amount_paid,
            );
        }
        insert_money(
            &mut fields,
            "sched4AmountPaid",
            self.schedule_4_amount_total(),
        );

        insert(&mut fields, "frm2550qv2024:txtCurrentPage", "2");
        insert(&mut fields, "frm2550qv2024:txtMaxPage", "2");
        for key in [
            "resultOtherCreditsNo19",
            "resultOtherCreditsNo42",
            "resultOtherCreditsNo47",
            "resultOtherCreditsNo56",
        ] {
            fields
                .entry(key.to_string())
                .or_insert_with(|| "0.00".to_string());
        }
        insert_money(
            &mut fields,
            "txtTotalAmountOfBalanceofInputTaxFromPrevious",
            self.schedule_1_previous_total(),
        );
        insert_money(
            &mut fields,
            "txtTotalAmountOfBalanceofInputTaxToBeCarried",
            self.schedule_1_next_total(),
        );
        insert_money(
            &mut fields,
            "txtTotalAmountofIncomePayment",
            self.schedule_3_income_total(),
        );
        insert_money(
            &mut fields,
            "txtTotalAmoungOfTaxWithHeld",
            self.schedule_3_tax_total(),
        );
        insert_money(
            &mut fields,
            "txtAmountPaidSched4",
            self.schedule_4_amount_total(),
        );

        insert(
            &mut fields,
            "txtFinalFlag",
            self.xml_final_flag.as_xml_value(),
        );
        insert(&mut fields, "txtEnroll", "Y");
        insert(&mut fields, "ebirOnlineConfirmUsername", "");
        insert(&mut fields, "ebirOnlineUsername", "");
        insert(&mut fields, "ebirOnlineSecret", "");
        // One email on 2550Q (maintainer decision 2026-10-17): Item 12's
        // taxpayerEmailAddress. txtEmail is the HTA's hidden duplicate, which
        // the official save leaves blank (tests/sample-payloads/
        // 2550Q-122025Q1.official.xml: `txtEmail=txtEmail=`), so a value an
        // import or an older draft kept in `xml_contact_email` is never filed.
        insert(&mut fields, "txtEmail", "");
        insert(&mut fields, "driveSelectTPExport", "0");
        insert(
            &mut fields,
            "dateFiled",
            self.date_filed
                .map(Form2550QDate::to_filed_date)
                .unwrap_or_default(),
        );
        assert_eq!(
            fields.len(),
            EXACT_SOURCE_FIELD_COUNT,
            "2550Q reviewed field map must contain exactly 160 fields"
        );
        assert_eq!(
            fields.keys().cloned().collect::<BTreeSet<_>>(),
            expected,
            "2550Q reviewed field map must match the exact reviewed key set"
        );
        fields
    }

    #[cfg(test)]
    pub(crate) fn to_bir_xml_payload(&self) -> String {
        crate::bir_xml::generate_bir_xml(&self.to_bir_field_map())
    }

    /// Produce an editable-save payload only after semantic and evidence gates
    /// pass. This is not a queue/submission API.
    pub fn try_to_bir_xml_payload(&self) -> Result<String, Vec<(String, String)>> {
        let mut normalized = self.clone();
        normalized.recompute();
        let mut errors = normalized.validate();
        // Checked XML never synthesizes a required raw value from typed state.
        // This binds here, at the point XML is produced, rather than in
        // `validate()`, which also serves print preview.
        errors.extend(normalized.missing_required_raw_authority_errors());
        for (field, rows) in [
            (
                "item_19_additional_rows",
                &normalized.item_19_additional_rows,
            ),
            (
                "item_42_additional_rows",
                &normalized.item_42_additional_rows,
            ),
            (
                "item_47_additional_rows",
                &normalized.item_47_additional_rows,
            ),
            (
                "item_56_additional_rows",
                &normalized.item_56_additional_rows,
            ),
        ] {
            if !rows.is_empty() {
                errors.push((
                    field.to_string(),
                    "Popup rows are written only by the official upload \
                     (QueueableForm::official_payload), not by this reviewed projection"
                        .to_string(),
                ));
            }
        }
        if errors.is_empty() {
            Ok(crate::bir_xml::generate_bir_xml(
                &normalized.to_reviewed_field_map(),
            ))
        } else {
            Err(errors)
        }
    }

    pub fn from_bir_xml_payload(xml: &str) -> Result<Self, Vec<(String, String)>> {
        let mut fields = crate::bir_xml::parse_bir_xml_checked(xml).map_err(|error| {
            vec![(
                "xml_payload".to_string(),
                format!("Invalid 2550Q pseudo-XML: {error}"),
            )]
        })?;
        if !fields.contains_key("dateFiled")
            && let Some(value) = standalone_element(xml, "dateFiled")
        {
            fields.insert("dateFiled".to_string(), value);
        }
        Self::from_bir_field_map(&fields)
    }

    pub fn from_bir_field_map(
        fields: &BTreeMap<String, String>,
    ) -> Result<Self, Vec<(String, String)>> {
        let mut errors = Vec::new();
        let expected = expected_xml_keys();
        for key in fields.keys() {
            if !expected.contains(key) {
                errors.push((
                    key.clone(),
                    format!("Unreviewed 2550Q source field {key} is not allowed"),
                ));
            }
        }
        for key in &expected {
            if !fields.contains_key(key) {
                errors.push((
                    key.clone(),
                    format!("Required 2550Q source field {key} is missing"),
                ));
            }
        }

        require_exact_value(fields, "txtEnroll", "Y", &mut errors);
        require_exact_value(fields, "driveSelectTPExport", "0", &mut errors);
        require_exact_value(fields, "frm2550qv2024:txtCurrentPage", "2", &mut errors);
        require_exact_value(fields, "frm2550qv2024:txtMaxPage", "2", &mut errors);
        for key in [
            "ebirOnlineConfirmUsername",
            "ebirOnlineUsername",
            "ebirOnlineSecret",
        ] {
            require_exact_value(fields, key, "", &mut errors);
        }

        let filing_basis = parse_bool_pair(
            fields,
            "frm2550qv2024:calendarNo1",
            "frm2550qv2024:fiscalNo1",
            "filing_basis",
            &mut errors,
        )
        .map(|calendar| {
            if calendar {
                Form2550QFilingBasis::Calendar
            } else {
                Form2550QFilingBasis::Fiscal
            }
        });
        let quarter_index = parse_one_of(
            fields,
            &[
                "frm2550qv2024:OptQuarter1",
                "frm2550qv2024:OptQuarter2",
                "frm2550qv2024:OptQuarter3",
                "frm2550qv2024:OptQuarter4",
            ],
            "quarter",
            &mut errors,
        );
        let is_amended = parse_bool_pair(
            fields,
            "frm2550qv2024:amendedReturnYesNo5",
            "frm2550qv2024:amendedReturnNo5",
            "is_amended",
            &mut errors,
        );
        let is_short_period_return = parse_bool_pair(
            fields,
            "frm2550qv2024:OptShortPrd1",
            "frm2550qv2024:OptShortPrd2",
            "is_short_period_return",
            &mut errors,
        );
        let classification_index = parse_one_of(
            fields,
            &[
                "frm2550qv2024:taxPayerClassification1",
                "frm2550qv2024:taxPayerClassification2",
                "frm2550qv2024:taxPayerClassification3",
                "frm2550qv2024:taxPayerClassification4",
            ],
            "taxpayer_classification",
            &mut errors,
        );
        let is_availing_tax_relief = parse_bool_pair(
            fields,
            "frm2550qv2024:internationalTreatyYn",
            "frm2550qv2024:specialRateYn",
            "is_availing_tax_relief",
            &mut errors,
        );

        let tin = format!(
            "{}{}{}{}",
            field(fields, "frm2550qv2024:txtTIN1"),
            field(fields, "frm2550qv2024:txtTIN2"),
            field(fields, "frm2550qv2024:txtTIN3"),
            field(fields, "frm2550qv2024:branchCode")
        );
        verify_duplicate(
            fields,
            "frm2550qv2024:txtTIN1",
            "frm2550qv2024:txtPg2TIN1",
            &mut errors,
        );
        verify_duplicate(
            fields,
            "frm2550qv2024:txtTIN2",
            "frm2550qv2024:txtPg2TIN2",
            &mut errors,
        );
        verify_duplicate(
            fields,
            "frm2550qv2024:txtTIN3",
            "frm2550qv2024:txtPg2TIN3",
            &mut errors,
        );
        verify_duplicate(
            fields,
            "frm2550qv2024:branchCode",
            "frm2550qv2024:txtPg2BranchCode",
            &mut errors,
        );
        verify_duplicate(
            fields,
            "frm2550qv2024:taxpayerName",
            "frm2550qv2024:Pg2TaxPayer",
            &mut errors,
        );

        let schedule_1 = SCHEDULE_ROW_SUFFIXES
            .into_iter()
            .map(|suffix| parse_capital_good_row(fields, suffix, &mut errors))
            .collect::<Vec<_>>();
        let schedule_3 = SCHEDULE_3_SUFFIXES
            .into_iter()
            .map(|suffix| parse_creditable_vat_row(fields, suffix, &mut errors))
            .collect::<Vec<_>>();
        let schedule_4 = SCHEDULE_4_SUFFIXES
            .into_iter()
            .map(|suffix| parse_advance_vat_row(fields, suffix, &mut errors))
            .collect::<Vec<_>>();

        let xml_final_flag = match field(fields, "txtFinalFlag") {
            "0" => Form2550QXmlFinalFlag::Zero,
            "1" => Form2550QXmlFinalFlag::One,
            "" => Form2550QXmlFinalFlag::Missing,
            value => Form2550QXmlFinalFlag::Unknown(value.to_string()),
        };

        if !errors.is_empty() {
            return Err(errors);
        }

        let mut draft = Form2550QDraft {
            id: None,
            tin,
            taxable_year: parse_required(fields, "frm2550qv2024:txtYearNo2", &mut errors)
                .unwrap_or_default(),
            filing_basis: filing_basis.unwrap_or_default(),
            year_end_month: parse_required(fields, "frm2550qv2024:selectedMonthNo2", &mut errors)
                .unwrap_or_default(),
            quarter: Form2550QQuarter::from_number(
                u8::try_from(quarter_index.unwrap_or_default()).unwrap_or_default(),
            ),
            return_period_from: parse_optional_date(
                fields,
                "frm2550qv2024:RtnPeriodFromNo4",
                DateFormat::ReturnPeriod,
                &mut errors,
            ),
            return_period_to: parse_optional_date(
                fields,
                "frm2550qv2024:RtnPeriodToNo4",
                DateFormat::ReturnPeriod,
                &mut errors,
            ),
            is_amended: is_amended.unwrap_or(false),
            is_short_period_return: is_short_period_return.unwrap_or(false),
            rdo_code: semantic_text(fields, "frm2550qv2024:txtRDOCode"),
            taxpayer_name: semantic_text(fields, "frm2550qv2024:taxpayerName"),
            registered_address: semantic_text(fields, "frm2550qv2024:taxpayerAddress"),
            zip_code: semantic_text(fields, "frm2550qv2024:taxpayerZip"),
            contact_number: semantic_text(fields, "frm2550qv2024:taxpayerContactNumber"),
            email: semantic_text(fields, "frm2550qv2024:taxpayerEmailAddress"),
            taxpayer_classification: classification_index.and_then(classification_from_index),
            is_availing_tax_relief: is_availing_tax_relief.unwrap_or(false),
            tax_relief_details: semantic_text(fields, "frm2550qv2024:specifyInternationalTreaty"),
            part_ii: Form2550QPartII {
                item_18_paid_on_previous_return: parse_money(
                    fields,
                    "frm2550qv2024:vatPaidReturn",
                    &mut errors,
                ),
                item_19_description: semantic_text(fields, "frm2550qv2024:addSpecifyNo19"),
                item_19_other_credit_or_payment: parse_money(
                    fields,
                    "frm2550qv2024:otherCreditsNo19",
                    &mut errors,
                ),
                item_22_surcharge: parse_money(fields, "frm2550qv2024:surcharge", &mut errors),
                item_23_interest: parse_money(fields, "frm2550qv2024:interest", &mut errors),
                item_24_compromise: parse_money(fields, "frm2550qv2024:compromise", &mut errors),
                ..Form2550QPartII::default()
            },
            part_iv: Form2550QPartIV {
                item_31a_vatable_sales: parse_money(
                    fields,
                    "frm2550qv2024:vatableSales",
                    &mut errors,
                ),
                item_32a_zero_rated_sales: parse_money(
                    fields,
                    "frm2550qv2024:zeroRatedSales",
                    &mut errors,
                ),
                item_33a_exempt_sales: parse_money(
                    fields,
                    "frm2550qv2024:exemptSales",
                    &mut errors,
                ),
                item_35b_less_output_vat_uncollected: parse_money(
                    fields,
                    "frm2550qv2024:lessOutputVat",
                    &mut errors,
                ),
                item_36b_add_output_vat_recovered: parse_money(
                    fields,
                    "frm2550qv2024:addOutputVat",
                    &mut errors,
                ),
                item_38b_input_tax_carried: parse_money(
                    fields,
                    "frm2550qv2024:inputTaxCarried",
                    &mut errors,
                ),
                item_40b_transitional_input_tax: parse_money(
                    fields,
                    "frm2550qv2024:transitionalInputTax",
                    &mut errors,
                ),
                item_41b_presumptive_input_tax: parse_money(
                    fields,
                    "frm2550qv2024:presumptiveInputTax",
                    &mut errors,
                ),
                item_42_description: semantic_text(fields, "frm2550qv2024:addSpecifyNo42"),
                item_42b_other_input_tax: parse_money(
                    fields,
                    "frm2550qv2024:otherSpecify42",
                    &mut errors,
                ),
                item_44a_domestic_purchases: parse_money(
                    fields,
                    "frm2550qv2024:domesticPurchase",
                    &mut errors,
                ),
                item_44b_domestic_input_tax: parse_money(
                    fields,
                    "frm2550qv2024:domesticInputTax",
                    &mut errors,
                ),
                item_45a_nonresident_services: parse_money(
                    fields,
                    "frm2550qv2024:servicesPurchase",
                    &mut errors,
                ),
                item_45b_nonresident_service_input_tax: parse_money(
                    fields,
                    "frm2550qv2024:serviceInputTax",
                    &mut errors,
                ),
                item_46a_importations: parse_money(
                    fields,
                    "frm2550qv2024:importPurchase",
                    &mut errors,
                ),
                item_46b_import_input_tax: parse_money(
                    fields,
                    "frm2550qv2024:importInputTax",
                    &mut errors,
                ),
                item_47_description: semantic_text(fields, "frm2550qv2024:addSpecifyNo47"),
                item_47a_other_purchases: parse_money(
                    fields,
                    "frm2550qv2024:otherSpecify47",
                    &mut errors,
                ),
                item_47b_other_input_tax: parse_money(fields, "otherSpecify47B", &mut errors),
                item_48a_domestic_purchases_no_input_tax: parse_money(
                    fields,
                    "frm2550qv2024:domesticPurchaseNoTax",
                    &mut errors,
                ),
                item_49a_vat_exempt_importations: parse_money(
                    fields,
                    "frm2550qv2024:vatExemptImports",
                    &mut errors,
                ),
                item_54b_vat_refund_or_tcc_claimed: parse_money(
                    fields,
                    "frm2550qv2024:vatRefund",
                    &mut errors,
                ),
                item_55b_input_vat_on_unpaid_payables: parse_money(
                    fields,
                    "frm2550qv2024:inputVatUnpaid",
                    &mut errors,
                ),
                item_56_description: semantic_text(fields, "frm2550qv2024:addSpecifyNo56"),
                item_56b_other_deduction: parse_money(
                    fields,
                    "frm2550qv2024:otherSpecify56",
                    &mut errors,
                ),
                item_58b_input_vat_on_settled_payables: parse_money(
                    fields,
                    "frm2550qv2024:addInputVat",
                    &mut errors,
                ),
                ..Form2550QPartIV::default()
            },
            schedule_1,
            schedule_2: Form2550QSchedule2 {
                input_tax_directly_attributable_to_exempt_sales: parse_money(
                    fields,
                    "frm2550qv2024:sched2InputTaxDirect",
                    &mut errors,
                ),
                vat_exempt_sales: parse_money(
                    fields,
                    "frm2550qv2024:sched2VatExemptSale",
                    &mut errors,
                ),
                input_tax_not_directly_attributable: parse_money(
                    fields,
                    "frm2550qv2024:sched2AmountInputTax",
                    &mut errors,
                ),
                ..Form2550QSchedule2::default()
            },
            schedule_3,
            schedule_4,
            item_19_additional_rows: Vec::new(),
            item_42_additional_rows: Vec::new(),
            item_47_additional_rows: Vec::new(),
            item_56_additional_rows: Vec::new(),
            row_identity_state: Default::default(),
            raw_editor_state: Default::default(),
            local_print_fields: Form2550QLocalPrintFields::default(),
            xml_final_flag,
            xml_contact_email: semantic_text(fields, "txtEmail"),
            date_filed: parse_optional_date(fields, "dateFiled", DateFormat::Filed, &mut errors),
            preserved_unmodeled_xml_fields: fields
                .iter()
                .filter(|(key, _)| is_preserved_unmodeled_xml_key(key))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
            migration_review_items: Vec::new(),
            legacy_flat_draft_fields: BTreeMap::new(),
            lifecycle: super::queueable::SubmissionLifecycle::default(),
        };

        if !errors.is_empty() {
            return Err(errors);
        }

        if let Err(error) = seed_raw_editor_state_from_reviewed_fields(&mut draft, fields) {
            errors.push((
                "raw_editor_state".to_string(),
                format!("Unable to preserve reviewed 2550Q raw source fields: {error}"),
            ));
        }

        let source_computed = computed_source_values(fields, &mut errors);
        draft.recompute();
        for (key, actual) in computed_draft_values(&draft) {
            verify_computed_source(
                key,
                source_computed.get(key).copied().flatten(),
                actual,
                &mut errors,
            );
        }
        verify_computed_source(
            "txtTotalAmountOfBalanceofInputTaxFromPrevious",
            parse_money(
                fields,
                "txtTotalAmountOfBalanceofInputTaxFromPrevious",
                &mut errors,
            ),
            draft.schedule_1_previous_total(),
            &mut errors,
        );
        verify_computed_source(
            "txtTotalAmountOfBalanceofInputTaxToBeCarried",
            parse_money(
                fields,
                "txtTotalAmountOfBalanceofInputTaxToBeCarried",
                &mut errors,
            ),
            draft.schedule_1_next_total(),
            &mut errors,
        );
        verify_computed_source(
            "txtTotalAmountofIncomePayment",
            parse_money(fields, "txtTotalAmountofIncomePayment", &mut errors),
            draft.schedule_3_income_total(),
            &mut errors,
        );
        verify_computed_source(
            "txtTotalAmoungOfTaxWithHeld",
            parse_money(fields, "txtTotalAmoungOfTaxWithHeld", &mut errors),
            draft.schedule_3_tax_total(),
            &mut errors,
        );
        verify_computed_source(
            "txtAmountPaidSched4",
            parse_money(fields, "txtAmountPaidSched4", &mut errors),
            draft.schedule_4_amount_total(),
            &mut errors,
        );

        errors.extend(draft.validate());
        if errors.is_empty() {
            Ok(draft)
        } else {
            Err(errors)
        }
    }
}

fn singleton_raw_value_kind(field_key: &str) -> ReviewedRawValueKind {
    match field_key {
        "frm2550qv2024:calendarNo1"
        | "frm2550qv2024:fiscalNo1"
        | "frm2550qv2024:OptQuarter1"
        | "frm2550qv2024:OptQuarter2"
        | "frm2550qv2024:OptQuarter3"
        | "frm2550qv2024:OptQuarter4"
        | "frm2550qv2024:amendedReturnYesNo5"
        | "frm2550qv2024:amendedReturnNo5"
        | "frm2550qv2024:OptShortPrd1"
        | "frm2550qv2024:OptShortPrd2"
        | "frm2550qv2024:taxPayerClassification1"
        | "frm2550qv2024:taxPayerClassification2"
        | "frm2550qv2024:taxPayerClassification3"
        | "frm2550qv2024:taxPayerClassification4"
        | "frm2550qv2024:internationalTreatyYn"
        | "frm2550qv2024:specialRateYn" => ReviewedRawValueKind::Boolean,
        "frm2550qv2024:selectedMonthNo2" => ReviewedRawValueKind::RequiredU8,
        "frm2550qv2024:txtYearNo2" => ReviewedRawValueKind::RequiredU16,
        "frm2550qv2024:RtnPeriodFromNo4" | "frm2550qv2024:RtnPeriodToNo4" => {
            ReviewedRawValueKind::Date
        }
        "frm2550qv2024:specifyInternationalTreaty"
        | "frm2550qv2024:txtTIN1"
        | "frm2550qv2024:txtTIN2"
        | "frm2550qv2024:txtTIN3"
        | "frm2550qv2024:branchCode"
        | "frm2550qv2024:txtRDOCode"
        | "frm2550qv2024:taxpayerName"
        | "frm2550qv2024:taxpayerAddress"
        | "frm2550qv2024:taxpayerZip"
        | "frm2550qv2024:taxpayerContactNumber"
        | "frm2550qv2024:taxpayerEmailAddress"
        | "frm2550qv2024:addSpecifyNo19"
        | "frm2550qv2024:addSpecifyNo42"
        | "frm2550qv2024:addSpecifyNo47"
        | "frm2550qv2024:addSpecifyNo56" => ReviewedRawValueKind::Text,
        _ => ReviewedRawValueKind::Money,
    }
}

fn repeated_raw_value_kind(family: Form2550QRowFamily, member_key: &str) -> ReviewedRawValueKind {
    match (family, member_key) {
        (Form2550QRowFamily::Schedule1, "txtDatePurchase1")
        | (Form2550QRowFamily::Schedule3, "txtDateCovered3")
        | (Form2550QRowFamily::Schedule3, "txtDateCovered3To")
        | (Form2550QRowFamily::Schedule4, "txtDate4")
        | (Form2550QRowFamily::Schedule4, "txtDate4To") => ReviewedRawValueKind::Date,
        (Form2550QRowFamily::Schedule1, "txtEstimatedLife1")
        | (Form2550QRowFamily::Schedule1, "txtRecognizedLife1") => {
            ReviewedRawValueKind::OptionalU16
        }
        (Form2550QRowFamily::Schedule1, "txtSourceCode1")
        | (Form2550QRowFamily::Schedule1, "txtDescription1")
        | (Form2550QRowFamily::Schedule3, "txtNameWithHoldingAgent3")
        | (Form2550QRowFamily::Schedule4, "txtNameOfMiller4")
        | (Form2550QRowFamily::Schedule4, "txtNameOfTaxpayer4") => ReviewedRawValueKind::Text,
        (Form2550QRowFamily::Schedule4, "txtOfficialReceiptNumber4") => {
            ReviewedRawValueKind::ReceiptIdentifier
        }
        (Form2550QRowFamily::Schedule1, "txtAmountPurchase1")
        | (Form2550QRowFamily::Schedule1, "txtInputTax1")
        | (Form2550QRowFamily::Schedule1, "txtAllowedInputTax1")
        | (Form2550QRowFamily::Schedule1, "txtBalanceInputTax1")
        | (Form2550QRowFamily::Schedule3, "txtIncomePayment3")
        | (Form2550QRowFamily::Schedule3, "txtTotalTaxWithHeld3")
        | (Form2550QRowFamily::Schedule4, "txtAmountPaid4") => ReviewedRawValueKind::Money,
        _ => panic!(
            "reviewed 2550Q raw binding inventory has no semantic kind for {} member {member_key}",
            family.label()
        ),
    }
}

fn raw_family_path(family: Form2550QRowFamily) -> &'static str {
    match family {
        Form2550QRowFamily::Schedule1 => "schedule-1",
        Form2550QRowFamily::Schedule3 => "schedule-3",
        Form2550QRowFamily::Schedule4 => "schedule-4",
        Form2550QRowFamily::Item19Additional => "item-19-additional",
        Form2550QRowFamily::Item42Additional => "item-42-additional",
        Form2550QRowFamily::Item47Additional => "item-47-additional",
        Form2550QRowFamily::Item56Additional => "item-56-additional",
    }
}

fn raw_matches_reviewed_value(
    raw: &RawValue,
    expected: &str,
    kind: ReviewedRawValueKind,
) -> Result<bool, ()> {
    let raw_text = raw.as_text();
    match kind {
        ReviewedRawValueKind::Boolean => {
            Ok(parse_boolean_buffer(raw_text)? == parse_boolean_buffer(Some(expected))?)
        }
        ReviewedRawValueKind::Text => {
            Ok(normalize_optional_text(raw_text) == normalize_optional_text(Some(expected)))
        }
        ReviewedRawValueKind::Money => {
            Ok(parse_optional_money_buffer(raw_text)?
                == parse_optional_money_buffer(Some(expected))?)
        }
        ReviewedRawValueKind::Date => Ok(
            parse_optional_date_buffer(raw_text)? == parse_optional_date_buffer(Some(expected))?
        ),
        ReviewedRawValueKind::RequiredU8 => {
            Ok(parse_required_u8_buffer(raw_text)? == parse_required_u8_buffer(Some(expected))?)
        }
        ReviewedRawValueKind::RequiredU16 => {
            Ok(parse_required_u16_buffer(raw_text)? == parse_required_u16_buffer(Some(expected))?)
        }
        ReviewedRawValueKind::OptionalU16 => {
            Ok(parse_optional_u16_buffer(raw_text)? == parse_optional_u16_buffer(Some(expected))?)
        }
        ReviewedRawValueKind::ReceiptIdentifier => {
            Ok(normalize_receipt_identifier(raw_text)
                == normalize_receipt_identifier(Some(expected)))
        }
    }
}

fn parse_boolean_buffer(value: Option<&str>) -> Result<bool, ()> {
    match value.ok_or(())? {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(()),
    }
}

fn normalize_optional_text(value: Option<&str>) -> Option<&str> {
    value.and_then(|text| (!text.is_empty()).then_some(text))
}

fn parse_optional_money_buffer(value: Option<&str>) -> Result<Option<f64>, ()> {
    let Some(text) = value else {
        return Ok(None);
    };
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let value = trimmed.replace(',', "").parse::<f64>().map_err(|_| ())?;
    value.is_finite().then_some(Some(value)).ok_or(())
}

fn parse_optional_date_buffer(value: Option<&str>) -> Result<Option<Form2550QDate>, ()> {
    let Some(text) = value else {
        return Ok(None);
    };
    if text.trim().is_empty() {
        return Ok(None);
    }
    Form2550QDate::parse_return_period(text)
        .map(Some)
        .map_err(|_| ())
}

fn parse_required_u8_buffer(value: Option<&str>) -> Result<u8, ()> {
    value.ok_or(())?.trim().parse::<u8>().map_err(|_| ())
}

fn parse_required_u16_buffer(value: Option<&str>) -> Result<u16, ()> {
    value.ok_or(())?.trim().parse::<u16>().map_err(|_| ())
}

fn parse_optional_u16_buffer(value: Option<&str>) -> Result<Option<u16>, ()> {
    let Some(text) = value else {
        return Ok(None);
    };
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let value = trimmed.replace(',', "").parse::<f64>().map_err(|_| ())?;
    if !value.is_finite() || value < 0.0 || value.fract() != 0.0 || value > f64::from(u16::MAX) {
        return Err(());
    }
    Ok(Some(value as u16))
}

fn normalize_receipt_identifier(value: Option<&str>) -> Option<&str> {
    value.and_then(|text| {
        let trimmed = text.trim();
        (!trimmed.is_empty() && trimmed != "0.00").then_some(text)
    })
}

fn computed_source_values(
    fields: &BTreeMap<String, String>,
    errors: &mut Vec<(String, String)>,
) -> BTreeMap<&'static str, Option<f64>> {
    COMPUTED_XML_KEYS
        .iter()
        .map(|key| (*key, parse_money(fields, key, errors)))
        .collect()
}

fn computed_draft_values(draft: &Form2550QDraft) -> Vec<(&'static str, Option<f64>)> {
    let p2 = &draft.part_ii;
    let p4 = &draft.part_iv;
    let s2 = &draft.schedule_2;
    vec![
        (
            "frm2550qv2024:excessInputTax",
            p2.item_15_net_vat_payable_or_excess,
        ),
        (
            "frm2550qv2024:creditableVat",
            p2.item_16_creditable_vat_withheld,
        ),
        (
            "frm2550qv2024:advVatPayment",
            p2.item_17_advance_vat_payments,
        ),
        (
            "frm2550qv2024:totalTaxCredits",
            p2.item_20_total_credits_or_payments,
        ),
        (
            "frm2550qv2024:excessCredits",
            p2.item_21_tax_payable_or_excess_credits,
        ),
        ("frm2550qv2024:penalties", p2.item_25_total_penalties),
        (
            "frm2550qv2024:totalPayable",
            p2.item_26_total_amount_payable_or_excess,
        ),
        ("frm2550qv2024:outputVatSales", p4.item_31b_output_tax),
        ("frm2550qv2024:totalSales", p4.item_34a_total_sales),
        ("frm2550qv2024:outputTaxDue", p4.item_34b_output_tax_due),
        (
            "frm2550qv2024:totalAdjOutput",
            p4.item_37b_adjusted_output_tax_due,
        ),
        (
            "frm2550qv2024:inputTaxDeferred",
            p4.item_39b_input_tax_deferred,
        ),
        ("frm2550qv2024:total43", p4.item_43b_total_prior_input_tax),
        (
            "frm2550qv2024:totalCurPurchase",
            p4.item_50a_total_current_purchases,
        ),
        (
            "frm2550qv2024:totalCurInputTax",
            p4.item_50b_total_current_input_tax,
        ),
        (
            "frm2550qv2024:totalAvailInputTax",
            p4.item_51b_total_available_input_tax,
        ),
        (
            "frm2550qv2024:importCapitalInputTax",
            p4.item_52b_deferred_capital_goods_input_tax,
        ),
        (
            "frm2550qv2024:inputTaxAttr",
            p4.item_53b_input_tax_attributable_to_exempt_sales,
        ),
        (
            "frm2550qv2024:totalDeductions",
            p4.item_57b_total_deductions,
        ),
        (
            "frm2550qv2024:adjDeductions",
            p4.item_59b_adjusted_deductions,
        ),
        (
            "frm2550qv2024:totalAllowInputTax",
            p4.item_60b_total_allowable_input_tax,
        ),
        (
            "frm2550qv2024:netVatPayable",
            p4.item_61b_net_vat_payable_or_excess,
        ),
        ("sched1TotalBalPrev", draft.schedule_1_previous_total()),
        ("sched1TotalBalNext", draft.schedule_1_next_total()),
        ("frm2550qv2024:sched2TotalSales", s2.total_sales),
        ("frm2550qv2024:sched2TotalRatable", s2.ratable_input_tax),
        (
            "frm2550qv2024:sched2TotalAttr",
            s2.total_input_tax_attributable_to_exempt_sales,
        ),
        ("sched3TotalIncome", draft.schedule_3_income_total()),
        ("sched3TotalTax", draft.schedule_3_tax_total()),
        ("sched4AmountPaid", draft.schedule_4_amount_total()),
    ]
}

const COMPUTED_XML_KEYS: [&str; 30] = [
    "frm2550qv2024:excessInputTax",
    "frm2550qv2024:creditableVat",
    "frm2550qv2024:advVatPayment",
    "frm2550qv2024:totalTaxCredits",
    "frm2550qv2024:excessCredits",
    "frm2550qv2024:penalties",
    "frm2550qv2024:totalPayable",
    "frm2550qv2024:outputVatSales",
    "frm2550qv2024:totalSales",
    "frm2550qv2024:outputTaxDue",
    "frm2550qv2024:totalAdjOutput",
    "frm2550qv2024:inputTaxDeferred",
    "frm2550qv2024:total43",
    "frm2550qv2024:totalCurPurchase",
    "frm2550qv2024:totalCurInputTax",
    "frm2550qv2024:totalAvailInputTax",
    "frm2550qv2024:importCapitalInputTax",
    "frm2550qv2024:inputTaxAttr",
    "frm2550qv2024:totalDeductions",
    "frm2550qv2024:adjDeductions",
    "frm2550qv2024:totalAllowInputTax",
    "frm2550qv2024:netVatPayable",
    "sched1TotalBalPrev",
    "sched1TotalBalNext",
    "frm2550qv2024:sched2TotalSales",
    "frm2550qv2024:sched2TotalRatable",
    "frm2550qv2024:sched2TotalAttr",
    "sched3TotalIncome",
    "sched3TotalTax",
    "sched4AmountPaid",
];

fn parse_capital_good_row(
    fields: &BTreeMap<String, String>,
    suffix: u8,
    errors: &mut Vec<(String, String)>,
) -> Form2550QCapitalGoodRow {
    Form2550QCapitalGoodRow {
        instance_id: None,
        purchase_or_import_date: parse_optional_date(
            fields,
            &format!("txtDatePurchase{suffix}"),
            DateFormat::ReturnPeriod,
            errors,
        ),
        source_code: semantic_text(fields, &format!("txtSourceCode{suffix}")),
        description: semantic_text(fields, &format!("txtDescription{suffix}")),
        purchase_or_import_amount: parse_money(
            fields,
            &format!("txtAmountPurchase{suffix}"),
            errors,
        ),
        input_tax: parse_money(fields, &format!("txtInputTax{suffix}"), errors),
        estimated_life_months: parse_life(fields, &format!("txtEstimatedLife{suffix}"), errors),
        recognized_life_months: parse_life(fields, &format!("txtRecognizedLife{suffix}"), errors),
        allowable_input_tax_for_period: parse_money(
            fields,
            &format!("txtAllowedInputTax{suffix}"),
            errors,
        ),
        balance_to_next_period: parse_money(fields, &format!("txtBalanceInputTax{suffix}"), errors),
    }
}

fn parse_creditable_vat_row(
    fields: &BTreeMap<String, String>,
    suffix: u8,
    errors: &mut Vec<(String, String)>,
) -> Form2550QCreditableVatRow {
    Form2550QCreditableVatRow {
        instance_id: None,
        period_from: parse_optional_date(
            fields,
            &format!("txtDateCovered3{suffix}"),
            DateFormat::ReturnPeriod,
            errors,
        ),
        period_to: parse_optional_date(
            fields,
            &format!("txtDateCovered3To{suffix}"),
            DateFormat::ReturnPeriod,
            errors,
        ),
        withholding_agent_name: semantic_text(fields, &format!("txtNameWithHoldingAgent3{suffix}")),
        income_payment: parse_money(fields, &format!("txtIncomePayment3{suffix}"), errors),
        tax_withheld: parse_money(fields, &format!("txtTotalTaxWithHeld3{suffix}"), errors),
    }
}

fn parse_advance_vat_row(
    fields: &BTreeMap<String, String>,
    suffix: u8,
    errors: &mut Vec<(String, String)>,
) -> Form2550QAdvanceVatRow {
    Form2550QAdvanceVatRow {
        instance_id: None,
        period_from: parse_optional_date(
            fields,
            &format!("txtDate4{suffix}"),
            DateFormat::ReturnPeriod,
            errors,
        ),
        period_to: parse_optional_date(
            fields,
            &format!("txtDate4To{suffix}"),
            DateFormat::ReturnPeriod,
            errors,
        ),
        miller_name: semantic_text(fields, &format!("txtNameOfMiller4{suffix}")),
        taxpayer_name: semantic_text(fields, &format!("txtNameOfTaxpayer4{suffix}")),
        official_receipt_number: semantic_text(
            fields,
            &format!("txtOfficialReceiptNumber4{suffix}"),
        ),
        amount_paid: parse_money(fields, &format!("txtAmountPaid4{suffix}"), errors),
    }
}

fn expected_xml_keys() -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    for key in [
        "frm2550qv2024:calendarNo1",
        "frm2550qv2024:fiscalNo1",
        "frm2550qv2024:selectedMonthNo2",
        "frm2550qv2024:txtYearNo2",
        "frm2550qv2024:OptQuarter1",
        "frm2550qv2024:OptQuarter2",
        "frm2550qv2024:OptQuarter3",
        "frm2550qv2024:OptQuarter4",
        "frm2550qv2024:RtnPeriodFromNo4",
        "frm2550qv2024:RtnPeriodToNo4",
        "frm2550qv2024:amendedReturnYesNo5",
        "frm2550qv2024:amendedReturnNo5",
        "frm2550qv2024:OptShortPrd1",
        "frm2550qv2024:OptShortPrd2",
        "frm2550qv2024:txtTIN1",
        "frm2550qv2024:txtTIN2",
        "frm2550qv2024:txtTIN3",
        "frm2550qv2024:branchCode",
        "frm2550qv2024:txtRDOCode",
        "frm2550qv2024:taxpayerName",
        "frm2550qv2024:taxpayerAddress",
        "frm2550qv2024:taxpayerZip",
        "frm2550qv2024:taxpayerContactNumber",
        "frm2550qv2024:taxpayerEmailAddress",
        "frm2550qv2024:taxPayerClassification1",
        "frm2550qv2024:taxPayerClassification2",
        "frm2550qv2024:taxPayerClassification3",
        "frm2550qv2024:taxPayerClassification4",
        "frm2550qv2024:internationalTreatyYn",
        "frm2550qv2024:specialRateYn",
        "frm2550qv2024:specifyInternationalTreaty",
        "frm2550qv2024:excessInputTax",
        "frm2550qv2024:creditableVat",
        "frm2550qv2024:advVatPayment",
        "frm2550qv2024:vatPaidReturn",
        "frm2550qv2024:addSpecifyNo19",
        "frm2550qv2024:otherCreditsNo19",
        "frm2550qv2024:totalTaxCredits",
        "frm2550qv2024:excessCredits",
        "frm2550qv2024:surcharge",
        "frm2550qv2024:interest",
        "frm2550qv2024:compromise",
        "frm2550qv2024:penalties",
        "frm2550qv2024:totalPayable",
        "frm2550qv2024:txtPg2TIN1",
        "frm2550qv2024:txtPg2TIN2",
        "frm2550qv2024:txtPg2TIN3",
        "frm2550qv2024:txtPg2BranchCode",
        "frm2550qv2024:Pg2TaxPayer",
        "frm2550qv2024:vatableSales",
        "frm2550qv2024:outputVatSales",
        "frm2550qv2024:zeroRatedSales",
        "frm2550qv2024:exemptSales",
        "frm2550qv2024:totalSales",
        "frm2550qv2024:outputTaxDue",
        "frm2550qv2024:lessOutputVat",
        "frm2550qv2024:addOutputVat",
        "frm2550qv2024:totalAdjOutput",
        "frm2550qv2024:inputTaxCarried",
        "frm2550qv2024:inputTaxDeferred",
        "frm2550qv2024:transitionalInputTax",
        "frm2550qv2024:presumptiveInputTax",
        "frm2550qv2024:addSpecifyNo42",
        "frm2550qv2024:otherSpecify42",
        "frm2550qv2024:total43",
        "frm2550qv2024:domesticPurchase",
        "frm2550qv2024:domesticInputTax",
        "frm2550qv2024:servicesPurchase",
        "frm2550qv2024:serviceInputTax",
        "frm2550qv2024:importPurchase",
        "frm2550qv2024:importInputTax",
        "frm2550qv2024:addSpecifyNo47",
        "frm2550qv2024:otherSpecify47",
        "otherSpecify47B",
        "frm2550qv2024:domesticPurchaseNoTax",
        "frm2550qv2024:vatExemptImports",
        "frm2550qv2024:totalCurPurchase",
        "frm2550qv2024:totalCurInputTax",
        "frm2550qv2024:totalAvailInputTax",
        "frm2550qv2024:importCapitalInputTax",
        "frm2550qv2024:inputTaxAttr",
        "frm2550qv2024:vatRefund",
        "frm2550qv2024:inputVatUnpaid",
        "frm2550qv2024:addSpecifyNo56",
        "frm2550qv2024:otherSpecify56",
        "frm2550qv2024:totalDeductions",
        "frm2550qv2024:addInputVat",
        "frm2550qv2024:adjDeductions",
        "frm2550qv2024:totalAllowInputTax",
        "frm2550qv2024:netVatPayable",
        "sched1TotalBalPrev",
        "sched1TotalBalNext",
        "frm2550qv2024:sched2InputTaxDirect",
        "frm2550qv2024:sched2VatExemptSale",
        "frm2550qv2024:sched2AmountInputTax",
        "frm2550qv2024:sched2TotalSales",
        "frm2550qv2024:sched2TotalRatable",
        "frm2550qv2024:sched2TotalAttr",
        "sched3TotalIncome",
        "sched3TotalTax",
        "sched4AmountPaid",
        "frm2550qv2024:txtCurrentPage",
        "frm2550qv2024:txtMaxPage",
        "resultOtherCreditsNo19",
        "resultOtherCreditsNo42",
        "resultOtherCreditsNo47",
        "resultOtherCreditsNo56",
        "txtTotalAmountOfBalanceofInputTaxFromPrevious",
        "txtTotalAmountOfBalanceofInputTaxToBeCarried",
        "txtTotalAmountofIncomePayment",
        "txtTotalAmoungOfTaxWithHeld",
        "txtAmountPaidSched4",
        "txtFinalFlag",
        "txtEnroll",
        "ebirOnlineConfirmUsername",
        "ebirOnlineUsername",
        "ebirOnlineSecret",
        "txtEmail",
        "driveSelectTPExport",
        "dateFiled",
    ] {
        keys.insert(key.to_string());
    }
    for suffix in SCHEDULE_ROW_SUFFIXES {
        for prefix in [
            "txtDatePurchase",
            "txtSourceCode",
            "txtDescription",
            "txtAmountPurchase",
            "txtInputTax",
            "txtEstimatedLife",
            "txtRecognizedLife",
            "txtAllowedInputTax",
            "txtBalanceInputTax",
        ] {
            keys.insert(format!("{prefix}{suffix}"));
        }
    }
    for suffix in SCHEDULE_3_SUFFIXES {
        for prefix in [
            "txtDateCovered3",
            "txtDateCovered3To",
            "txtNameWithHoldingAgent3",
            "txtIncomePayment3",
            "txtTotalTaxWithHeld3",
        ] {
            keys.insert(format!("{prefix}{suffix}"));
        }
    }
    for suffix in SCHEDULE_4_SUFFIXES {
        for prefix in [
            "txtDate4",
            "txtDate4To",
            "txtNameOfMiller4",
            "txtNameOfTaxpayer4",
            "txtOfficialReceiptNumber4",
            "txtAmountPaid4",
        ] {
            keys.insert(format!("{prefix}{suffix}"));
        }
    }
    keys
}

fn standalone_element(xml: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = xml.find(&open)?.checked_add(open.len())?;
    let end = xml[start..].find(&close)?.checked_add(start)?;
    Some(xml[start..end].trim().to_string())
}

fn semantic_text(fields: &BTreeMap<String, String>, key: &str) -> String {
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

fn parse_required<T: std::str::FromStr>(
    fields: &BTreeMap<String, String>,
    key: &str,
    errors: &mut Vec<(String, String)>,
) -> Option<T> {
    let value = field(fields, key).trim();
    if value.is_empty() {
        errors.push((
            key.to_string(),
            format!("Required 2550Q field {key} is blank"),
        ));
        return None;
    }
    value
        .parse()
        .map_err(|_| {
            errors.push((
                key.to_string(),
                format!("2550Q field {key} has an invalid value"),
            ));
        })
        .ok()
}

fn parse_bool(
    fields: &BTreeMap<String, String>,
    key: &str,
    errors: &mut Vec<(String, String)>,
) -> Option<bool> {
    match field(fields, key).trim() {
        "true" => Some(true),
        "false" => Some(false),
        _ => {
            errors.push((
                key.to_string(),
                format!("2550Q field {key} must be true or false"),
            ));
            None
        }
    }
}

fn parse_bool_pair(
    fields: &BTreeMap<String, String>,
    first_key: &str,
    second_key: &str,
    label: &str,
    errors: &mut Vec<(String, String)>,
) -> Option<bool> {
    let first = parse_bool(fields, first_key, errors)?;
    let second = parse_bool(fields, second_key, errors)?;
    if first == second {
        errors.push((
            label.to_string(),
            format!("{first_key} and {second_key} must contain exactly one true value"),
        ));
        None
    } else {
        Some(first)
    }
}

fn parse_one_of(
    fields: &BTreeMap<String, String>,
    keys: &[&str],
    label: &str,
    errors: &mut Vec<(String, String)>,
) -> Option<usize> {
    let selected = keys
        .iter()
        .enumerate()
        .filter_map(|(index, key)| {
            parse_bool(fields, key, errors)
                .filter(|value| *value)
                .map(|_| index + 1)
        })
        .collect::<Vec<_>>();
    if selected.len() == 1 {
        selected.first().copied()
    } else {
        errors.push((
            label.to_string(),
            format!("{label} must contain exactly one selected XML option"),
        ));
        None
    }
}

fn parse_money(
    fields: &BTreeMap<String, String>,
    key: &str,
    errors: &mut Vec<(String, String)>,
) -> Option<f64> {
    let value = field(fields, key).trim();
    if value.is_empty() {
        return None;
    }
    let normalized = value.replace(',', "");
    match normalized.parse::<f64>() {
        Ok(amount) if amount.is_finite() => Some(amount),
        _ => {
            errors.push((
                key.to_string(),
                format!("2550Q field {key} is not a finite amount"),
            ));
            None
        }
    }
}

fn parse_life(
    fields: &BTreeMap<String, String>,
    key: &str,
    errors: &mut Vec<(String, String)>,
) -> Option<u16> {
    let value = parse_money(fields, key, errors)?;
    if value < 0.0 || value.fract() != 0.0 || value > f64::from(u16::MAX) {
        errors.push((
            key.to_string(),
            "Life in months must be a whole non-negative number".to_string(),
        ));
        None
    } else {
        Some(value as u16)
    }
}

#[derive(Clone, Copy)]
enum DateFormat {
    ReturnPeriod,
    Filed,
}

fn parse_optional_date(
    fields: &BTreeMap<String, String>,
    key: &str,
    format: DateFormat,
    errors: &mut Vec<(String, String)>,
) -> Option<Form2550QDate> {
    let value = field(fields, key).trim();
    if value.is_empty() {
        return None;
    }
    let parsed = match format {
        DateFormat::ReturnPeriod => Form2550QDate::parse_return_period(value),
        DateFormat::Filed => Form2550QDate::parse_filed_date(value),
    };
    parsed
        .map_err(|message| errors.push((key.to_string(), message)))
        .ok()
}

fn classification_from_index(index: usize) -> Option<Form2550QTaxpayerClassification> {
    match index {
        1 => Some(Form2550QTaxpayerClassification::Micro),
        2 => Some(Form2550QTaxpayerClassification::Small),
        3 => Some(Form2550QTaxpayerClassification::Medium),
        4 => Some(Form2550QTaxpayerClassification::Large),
        _ => None,
    }
}

fn require_exact_value(
    fields: &BTreeMap<String, String>,
    key: &str,
    expected: &str,
    errors: &mut Vec<(String, String)>,
) {
    if field(fields, key) != expected {
        errors.push((
            key.to_string(),
            format!("2550Q field {key} is outside reviewed value {expected:?}"),
        ));
    }
}

fn verify_duplicate(
    fields: &BTreeMap<String, String>,
    first: &str,
    second: &str,
    errors: &mut Vec<(String, String)>,
) {
    if semantic_text(fields, first) != semantic_text(fields, second) {
        errors.push((
            second.to_string(),
            format!("Duplicate fields {first} and {second} disagree"),
        ));
    }
}

fn verify_computed_source(
    key: &str,
    source: Option<f64>,
    computed: Option<f64>,
    errors: &mut Vec<(String, String)>,
) {
    match (source, computed) {
        (Some(source), Some(computed)) if (source - computed).abs() <= 0.005 => {}
        (None, None) => {}
        _ => errors.push((
            key.to_string(),
            format!("Source computed field {key} does not match the official formula"),
        )),
    }
}

fn split_tin(tin: &str) -> (String, String, String, String) {
    let digits: String = tin
        .chars()
        .filter(|character| character.is_ascii_digit())
        .collect();
    let segment = |start: usize, end: usize| -> String {
        digits
            .get(start..end.min(digits.len()))
            .unwrap_or("")
            .to_string()
    };
    (
        segment(0, 3),
        segment(3, 6),
        segment(6, 9),
        digits.get(9..).unwrap_or("").to_string(),
    )
}

fn insert(map: &mut BTreeMap<String, String>, key: &str, value: impl Into<String>) {
    map.insert(key.to_string(), value.into());
}

fn insert_bool(map: &mut BTreeMap<String, String>, key: &str, value: bool) {
    insert(map, key, if value { "true" } else { "false" });
}

fn insert_bool_pair(
    map: &mut BTreeMap<String, String>,
    true_key: &str,
    false_key: &str,
    value: bool,
) {
    insert_bool(map, true_key, value);
    insert_bool(map, false_key, !value);
}

fn insert_one_of_four(map: &mut BTreeMap<String, String>, prefix: &str, selected: Option<u8>) {
    for index in 1..=4 {
        insert_bool(map, &format!("{prefix}{index}"), selected == Some(index));
    }
}

fn insert_taxpayer_classification(
    map: &mut BTreeMap<String, String>,
    classification: Option<Form2550QTaxpayerClassification>,
) {
    for (index, candidate) in Form2550QTaxpayerClassification::ALL.into_iter().enumerate() {
        insert_bool(
            map,
            &format!("frm2550qv2024:taxPayerClassification{}", index + 1),
            classification == Some(candidate),
        );
    }
}

fn insert_optional_date(
    map: &mut BTreeMap<String, String>,
    key: &str,
    value: Option<Form2550QDate>,
) {
    insert(
        map,
        key,
        value.map(|date| date.to_string()).unwrap_or_default(),
    );
}

fn insert_money(map: &mut BTreeMap<String, String>, key: &str, value: Option<f64>) {
    insert(map, key, value.map(format_money).unwrap_or_default());
}

fn insert_life(map: &mut BTreeMap<String, String>, key: &str, value: Option<u16>) {
    insert(
        map,
        key,
        value
            .map(|months| format!("{:.2}", f64::from(months)))
            .unwrap_or_default(),
    );
}

fn format_money(value: f64) -> String {
    let sign = if value.is_sign_negative() { "-" } else { "" };
    let raw = format!("{:.2}", value.abs());
    let (integer, fraction) = raw.split_once('.').unwrap_or((&raw, "00"));
    let mut grouped_reversed = String::with_capacity(integer.len() + integer.len() / 3);

    for (index, character) in integer.chars().rev().enumerate() {
        if index > 0 && index % 3 == 0 {
            grouped_reversed.push(',');
        }
        grouped_reversed.push(character);
    }

    let grouped = grouped_reversed.chars().rev().collect::<String>();
    format!("{sign}{grouped}.{fraction}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::form_rules::{Form2550QCaptureGap, Form2550QFieldValueSource};
    use crate::profile::TaxpayerProfile;
    use bir_rules::RawValue;

    fn profile() -> TaxpayerProfile {
        serde_json::from_value(serde_json::json!({
            "id": null,
            "full_name": "JUAN DELA CRUZ",
            "tin": {
                "segment1": "000",
                "segment2": "000",
                "segment3": "000",
                "branch": "00000"
            },
            "rdo_code": "018",
            "line_of_business": "SOFTWARE DEVELOPMENT",
            "registered_address": "OLONGAPO",
            "zip_code": "2200",
            "phone": "09123456789",
            "email": "TAXPAYER@EXAMPLE.COM",
            "default_form_type": "2550Qv2024",
            "taxpayer_type": "Individual",
            "tax_classification": "SelfEmployed",
            "is_vat_registered": true,
            "eopt_tier": "Medium",
            "withholding_obligations": [],
            "excise_tax_liabilities": [],
            "atc_codes": [],
            "business_start_date": null,
            "birth_date": null,
            "registration_activity_status": "Active",
            "is_dormant_entity": false,
            "is_government_withholding_entity": false,
            "is_gpp_partner": false,
            "is_top_withholding_agent": false,
            "income_tax_elections": { "elections": {} },
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z"
        }))
        .expect("valid profile")
    }

    fn reviewed_sample_draft() -> Form2550QDraft {
        let mut draft = Form2550QDraft::new_from_profile(&profile(), 2025, 1);
        draft.date_filed = Some(Form2550QDate::new(2026, 5, 8).expect("date"));
        draft.part_iv.item_31a_vatable_sales = Some(1_000.0);
        draft.part_iv.item_32a_zero_rated_sales = Some(1_000.0);
        draft.part_iv.item_33a_exempt_sales = Some(1_000.0);
        draft.part_iv.item_35b_less_output_vat_uncollected = Some(1_000.0);
        draft.part_iv.item_36b_add_output_vat_recovered = Some(1_000.0);
        draft.part_iv.item_38b_input_tax_carried = Some(1_000.0);
        draft.part_iv.item_40b_transitional_input_tax = Some(1_000.0);
        draft.part_iv.item_41b_presumptive_input_tax = Some(1_000.0);
        draft.part_iv.item_42_description = "EXAMPLE2".to_string();
        draft.part_iv.item_42b_other_input_tax = Some(10_000.0);
        draft.part_iv.item_44a_domestic_purchases = Some(1_000.0);
        draft.part_iv.item_44b_domestic_input_tax = Some(120.0);
        draft.part_iv.item_45a_nonresident_services = Some(1_000.0);
        draft.part_iv.item_45b_nonresident_service_input_tax = Some(120.0);
        draft.part_iv.item_46a_importations = Some(10_000.0);
        draft.part_iv.item_46b_import_input_tax = Some(1_200.0);
        draft.part_iv.item_47_description = "EXAMPLE 3".to_string();
        draft.part_iv.item_47a_other_purchases = Some(1_000.0);
        draft.part_iv.item_47b_other_input_tax = Some(120.0);
        draft.part_iv.item_48a_domestic_purchases_no_input_tax = Some(1_000.0);
        draft.part_iv.item_49a_vat_exempt_importations = Some(1_000.0);
        draft.part_iv.item_54b_vat_refund_or_tcc_claimed = Some(1_000.0);
        draft.part_iv.item_55b_input_vat_on_unpaid_payables = Some(1_000.0);
        draft.part_iv.item_56_description = "EXAMPLE 4".to_string();
        draft.part_iv.item_56b_other_deduction = Some(1_000.0);
        draft.part_iv.item_58b_input_vat_on_settled_payables = Some(10_000.0);
        draft.schedule_2.vat_exempt_sales = Some(1_000.0);
        draft.part_ii.item_19_description = "RXAMPLE".to_string();
        draft.part_ii.item_19_other_credit_or_payment = Some(1_000.0);
        draft.part_ii.item_22_surcharge = Some(1_000.0);
        draft.part_ii.item_23_interest = Some(100.0);
        draft.part_ii.item_24_compromise = Some(200.0);
        for (field_key, raw) in [
            ("frm2550qv2024:txtYearNo2", "2025"),
            ("frm2550qv2024:calendarNo1", "true"),
            ("frm2550qv2024:fiscalNo1", "false"),
            ("frm2550qv2024:OptQuarter1", "true"),
            ("frm2550qv2024:OptQuarter2", "false"),
            ("frm2550qv2024:OptQuarter3", "false"),
            ("frm2550qv2024:OptQuarter4", "false"),
            ("frm2550qv2024:taxpayerAddress", "OLONGAPO"),
            ("frm2550qv2024:taxpayerZip", "2200"),
            ("frm2550qv2024:taxpayerContactNumber", "09123456789"),
            ("frm2550qv2024:taxpayerEmailAddress", "TAXPAYER@EXAMPLE.COM"),
            ("frm2550qv2024:taxPayerClassification1", "false"),
            ("frm2550qv2024:taxPayerClassification2", "false"),
            ("frm2550qv2024:taxPayerClassification3", "true"),
            ("frm2550qv2024:taxPayerClassification4", "false"),
            ("frm2550qv2024:internationalTreatyYn", "false"),
            ("frm2550qv2024:specialRateYn", "true"),
            ("frm2550qv2024:specifyInternationalTreaty", ""),
            ("frm2550qv2024:addSpecifyNo19", "RXAMPLE"),
            ("frm2550qv2024:otherCreditsNo19", "1000"),
            ("frm2550qv2024:addSpecifyNo42", "EXAMPLE2"),
            ("frm2550qv2024:otherSpecify42", "10000"),
            ("frm2550qv2024:addSpecifyNo47", "EXAMPLE 3"),
            ("frm2550qv2024:otherSpecify47", "1000"),
            ("frm2550qv2024:addSpecifyNo56", "EXAMPLE 4"),
            ("frm2550qv2024:otherSpecify56", "1000"),
        ] {
            draft
                .raw_editor_state
                .set_singleton(field_key, RawValue::Text(raw.to_string()));
        }
        draft.recompute();
        draft
    }

    #[test]
    fn reviewed_contract_contains_exactly_160_keys() {
        let fields = reviewed_sample_draft().to_bir_field_map();
        assert_eq!(expected_xml_keys().len(), EXACT_SOURCE_FIELD_COUNT);
        assert_eq!(fields.len(), EXACT_SOURCE_FIELD_COUNT);
        assert_eq!(
            fields.keys().cloned().collect::<BTreeSet<_>>(),
            expected_xml_keys()
        );
    }

    #[test]
    fn checked_xml_rejects_every_unreviewed_or_missing_identity_field() {
        let mut source = reviewed_sample_draft().to_bir_field_map();
        source.insert(
            "frm2550qv2024:unreviewedFutureField".to_string(),
            "must-not-survive".to_string(),
        );
        let import_errors =
            Form2550QDraft::from_bir_field_map(&source).expect_err("extra source key fails closed");
        assert!(
            import_errors
                .iter()
                .any(|(field, _)| { field == "frm2550qv2024:unreviewedFutureField" })
        );

        let mut draft = reviewed_sample_draft();
        draft.preserved_unmodeled_xml_fields.insert(
            "frm2550qv2024:unreviewedFutureField".to_string(),
            "must-not-survive".to_string(),
        );
        let export_errors = draft
            .try_to_bir_xml_payload()
            .expect_err("unknown persisted XML key fails checked export");
        assert!(export_errors.iter().any(|(field, _)| {
            field == "preserved_unmodeled_xml_fields.frm2550qv2024:unreviewedFutureField"
        }));

        let mut missing_identity = reviewed_sample_draft();
        missing_identity.schedule_1[0].instance_id = None;
        let identity_errors = missing_identity
            .try_to_bir_xml_payload()
            .expect_err("checked export requires every stable row ID");
        assert!(
            identity_errors
                .iter()
                .any(|(field, _)| field == "repeated_row_identity")
        );
    }

    /// One email (maintainer decision 2026-10-17): txtEmail files blank as
    /// the official save does (`<div>txtEmail=txtEmail=</div>` in
    /// tests/sample-payloads/2550Q-122025Q1.official.xml), even for an
    /// imported or older draft that kept a value in `xml_contact_email`.
    #[test]
    fn txt_email_files_blank_like_the_official_save() {
        let official = include_str!("../../tests/sample-payloads/2550Q-122025Q1.official.xml");
        assert!(official.contains("<div>txtEmail=txtEmail=</div>"));
        let mut fields = reviewed_sample_draft().to_bir_field_map();
        fields.insert("txtEmail".to_string(), "stale@example.com".to_string());
        let imported = Form2550QDraft::from_bir_field_map(&fields).expect("imports");
        assert_eq!(imported.xml_contact_email, "stale@example.com");
        let mut older = reviewed_sample_draft();
        older.xml_contact_email = "stale@example.com".to_string();
        for draft in [imported, older] {
            let map = draft.to_bir_field_map();
            assert_eq!(map["txtEmail"], "");
            assert_eq!(
                map["frm2550qv2024:taxpayerEmailAddress"],
                draft.email.trim(),
                "item 12 still files the email"
            );
            let xml = draft.to_bir_xml_payload();
            assert!(xml.contains("<div>txtEmail=txtEmail=</div>"));
            assert!(!xml.contains("stale@example.com"));
        }
    }

    #[test]
    fn reviewed_field_map_round_trips_without_losing_zero_or_negative_values() {
        let source = reviewed_sample_draft().to_bir_field_map();
        let imported = Form2550QDraft::from_bir_field_map(&source).expect("source imports");
        let output = imported.to_bir_field_map();
        assert_eq!(
            output.get("frm2550qv2024:netVatPayable"),
            Some(&"-1,440.00".to_string())
        );
        assert_eq!(output, source);
    }

    #[test]
    fn raw_state_and_stable_ids_do_not_change_reviewed_xml_keys_or_order() {
        let mut draft = reviewed_sample_draft();
        let baseline_map = draft.to_bir_field_map();
        let baseline_xml = draft.to_bir_xml_payload();
        draft.raw_editor_state.set_singleton(
            "frm2550qv2024:vatPaidReturn",
            RawValue::Text("lexical-buffer-only".to_string()),
        );
        draft
            .raw_editor_state
            .set_singleton("signatory", RawValue::Text("LOCAL PRINT ONLY".to_string()));
        draft.schedule_1[0].instance_id = Some(
            bir_rules::StableInstanceId::parse("row-00000000000000000999")
                .expect("valid strict row ID"),
        );

        assert_eq!(draft.to_bir_field_map(), baseline_map);
        assert_eq!(draft.to_bir_xml_payload(), baseline_xml);
        assert_eq!(
            draft.to_bir_field_map().keys().collect::<Vec<_>>(),
            baseline_map.keys().collect::<Vec<_>>()
        );
    }

    #[test]
    fn checked_xml_blocks_malformed_buffers_and_unprojected_additional_rows() {
        let mut malformed = reviewed_sample_draft();
        malformed.raw_editor_state.set_singleton(
            "frm2550qv2024:vatPaidReturn",
            RawValue::Text("not-money".to_string()),
        );
        malformed
            .raw_editor_state
            .mark_malformed("frm2550qv2024:vatPaidReturn", "not money");
        let malformed_errors = malformed
            .try_to_bir_xml_payload()
            .expect_err("malformed visible text blocks checked XML");
        assert!(
            malformed_errors
                .iter()
                .any(|(field, _)| field == "raw_editor.frm2550qv2024:vatPaidReturn")
        );

        let mut additional = reviewed_sample_draft();
        additional.item_42_additional_rows.push(
            super::super::form_2550q::Form2550QAdditionalItemRow {
                instance_id: None,
                description: "PRESERVE ME".to_string(),
                amount: Some(10.0),
            },
        );
        additional
            .ensure_repeating_row_ids()
            .expect("additional row receives identity");
        let additional_errors = additional
            .try_to_bir_xml_payload()
            .expect_err("unprojected group blocks checked XML");
        assert!(
            additional_errors
                .iter()
                .any(|(field, _)| field == "item_42_additional_rows")
        );
    }

    #[test]
    fn checked_xml_proves_captured_raw_typed_coherence_without_trusting_markers() {
        let mut malformed_without_marker = reviewed_sample_draft();
        malformed_without_marker.raw_editor_state.set_singleton(
            "frm2550qv2024:vatPaidReturn",
            RawValue::Text("not-money".to_string()),
        );
        let preserved_malformed = malformed_without_marker.raw_editor_state.clone();
        let malformed_errors = malformed_without_marker
            .try_to_bir_xml_payload()
            .expect_err("an omitted malformed marker must not bypass checked XML");
        assert!(malformed_errors.iter().any(|(field, message)| {
            field == "raw_editor.frm2550qv2024:vatPaidReturn"
                && message.contains("malformed even though no malformed marker")
        }));
        assert_eq!(
            malformed_without_marker.raw_editor_state, preserved_malformed,
            "checked export must be read-only and retain the exact malformed buffer"
        );

        let mut valid_but_conflicting = reviewed_sample_draft();
        valid_but_conflicting.raw_editor_state.set_singleton(
            "frm2550qv2024:vatPaidReturn",
            RawValue::Text("123.45".to_string()),
        );
        let preserved_conflict = valid_but_conflicting.raw_editor_state.clone();
        let conflict_errors = valid_but_conflicting
            .try_to_bir_xml_payload()
            .expect_err("a valid raw value that conflicts with typed state must fail closed");
        assert!(conflict_errors.iter().any(|(field, message)| {
            field == "raw_editor.frm2550qv2024:vatPaidReturn"
                && message.contains("disagrees with the typed draft value")
        }));
        assert_eq!(
            valid_but_conflicting.raw_editor_state, preserved_conflict,
            "checked export must not reconcile or discard conflicting raw text"
        );

        let mut semantically_equivalent = reviewed_sample_draft();
        semantically_equivalent.raw_editor_state.set_singleton(
            "frm2550qv2024:vatableSales",
            RawValue::Text("  1000.0  ".to_string()),
        );
        semantically_equivalent
            .try_to_bir_xml_payload()
            .expect("alternate valid money spelling remains coherent with typed value");
        assert_eq!(
            semantically_equivalent
                .raw_editor_state
                .singleton_value("frm2550qv2024:vatableSales"),
            Some(&RawValue::Text("  1000.0  ".to_string()))
        );
    }

    #[test]
    fn candidate_boolean_raw_buffers_use_strict_true_false_coherence() {
        let mut coherent = reviewed_sample_draft();
        for (field_key, raw) in [
            ("frm2550qv2024:calendarNo1", "true"),
            ("frm2550qv2024:fiscalNo1", "false"),
            ("frm2550qv2024:OptQuarter1", "true"),
            ("frm2550qv2024:OptQuarter2", "false"),
            ("frm2550qv2024:OptQuarter3", "false"),
            ("frm2550qv2024:OptQuarter4", "false"),
        ] {
            coherent
                .raw_editor_state
                .set_singleton(field_key, RawValue::Text(raw.to_string()));
        }
        coherent
            .try_to_bir_xml_payload()
            .expect("exact candidate booleans remain coherent with typed choices");

        coherent.raw_editor_state.set_singleton(
            "frm2550qv2024:calendarNo1",
            RawValue::Text("false".to_string()),
        );
        let conflict = coherent
            .try_to_bir_xml_payload()
            .expect_err("valid but contradictory raw boolean must fail closed");
        assert!(conflict.iter().any(|(field, message)| {
            field == "raw_editor.frm2550qv2024:calendarNo1"
                && message.contains("disagrees with the typed draft value")
        }));

        coherent.raw_editor_state.set_singleton(
            "frm2550qv2024:calendarNo1",
            RawValue::Text("TRUE".to_string()),
        );
        let malformed = coherent
            .try_to_bir_xml_payload()
            .expect_err("noncanonical boolean spelling must fail closed");
        assert!(malformed.iter().any(|(field, message)| {
            field == "raw_editor.frm2550qv2024:calendarNo1"
                && message.contains("malformed even though no malformed marker")
        }));

        coherent.raw_editor_state.set_singleton(
            "frm2550qv2024:calendarNo1",
            RawValue::Text(" true ".to_string()),
        );
        let whitespace = coherent
            .try_to_bir_xml_payload()
            .expect_err("whitespace-padded boolean spelling must fail closed");
        assert!(whitespace.iter().any(|(field, message)| {
            field == "raw_editor.frm2550qv2024:calendarNo1"
                && message.contains("malformed even though no malformed marker")
        }));
    }

    #[test]
    fn candidate_taxable_year_raw_buffer_is_required_and_typed_coherent() {
        let mut coherent = reviewed_sample_draft();
        coherent.raw_editor_state.set_singleton(
            "frm2550qv2024:txtYearNo2",
            RawValue::Text(" 2025 ".to_string()),
        );
        coherent
            .try_to_bir_xml_payload()
            .expect("a lexical variant of the captured year may match the typed value");

        coherent.raw_editor_state.set_singleton(
            "frm2550qv2024:txtYearNo2",
            RawValue::Text("2024".to_string()),
        );
        let conflict = coherent
            .try_to_bir_xml_payload()
            .expect_err("captured year cannot silently rewrite or be replaced by typed state");
        assert!(conflict.iter().any(|(field, message)| {
            field == "raw_editor.frm2550qv2024:txtYearNo2"
                && message.contains("disagrees with the typed draft value")
        }));

        coherent.raw_editor_state.set_singleton(
            "frm2550qv2024:txtYearNo2",
            RawValue::Text("20x5".to_string()),
        );
        let malformed = coherent
            .try_to_bir_xml_payload()
            .expect_err("malformed captured year must fail closed");
        assert!(malformed.iter().any(|(field, message)| {
            field == "raw_editor.frm2550qv2024:txtYearNo2"
                && message.contains("malformed even though no malformed marker")
        }));
    }

    #[test]
    fn candidate_profile_raw_buffers_are_required_exact_and_read_only() {
        let expected = [
            ("frm2550qv2024:taxpayerAddress", "OLONGAPO"),
            ("frm2550qv2024:taxpayerZip", "2200"),
            ("frm2550qv2024:taxpayerContactNumber", "09123456789"),
            ("frm2550qv2024:taxpayerEmailAddress", "TAXPAYER@EXAMPLE.COM"),
        ];
        let coherent = reviewed_sample_draft();
        for (field_key, raw) in expected {
            assert_eq!(
                coherent.raw_editor_state.singleton_value(field_key),
                Some(&RawValue::Text(raw.to_string()))
            );
        }
        coherent
            .try_to_bir_xml_payload()
            .expect("exact candidate profile buffers remain typed-coherent");

        let mut conflicting = reviewed_sample_draft();
        let typed_before = (
            conflicting.registered_address.clone(),
            conflicting.zip_code.clone(),
            conflicting.contact_number.clone(),
            conflicting.email.clone(),
        );
        conflicting.raw_editor_state.set_singleton(
            "frm2550qv2024:taxpayerAddress",
            RawValue::Text("OLONGAPO ".to_string()),
        );
        let raw_before = conflicting.raw_editor_state.clone();
        let errors = conflicting
            .try_to_bir_xml_payload()
            .expect_err("raw profile text must not be normalized or replaced by typed state");
        assert!(errors.iter().any(|(field, message)| {
            field == "raw_editor.frm2550qv2024:taxpayerAddress"
                && message.contains("disagrees with the typed draft value")
        }));
        assert_eq!(
            (
                conflicting.registered_address,
                conflicting.zip_code,
                conflicting.contact_number,
                conflicting.email,
            ),
            typed_before,
            "checked XML must not reconcile raw profile text into typed fields"
        );
        assert_eq!(
            conflicting.raw_editor_state, raw_before,
            "checked XML must retain conflicting raw profile text unchanged"
        );
    }

    #[test]
    fn checked_xml_never_synthesizes_missing_candidate_raw_values() {
        let missing = Form2550QDraft::new_from_profile(&profile(), 2025, 1);
        let errors = missing
            .try_to_bir_xml_payload()
            .expect_err("missing candidate raw authority must block checked XML");
        for field_key in REQUIRED_RAW_AUTHORITY_KEYS {
            assert!(errors.iter().any(|(field, message)| {
                field == &format!("raw_editor.{field_key}")
                    && message.contains("refuses to synthesize it from the typed draft")
            }));
        }

        let mut partial = Form2550QDraft::new_from_profile(&profile(), 2025, 1);
        partial.raw_editor_state.set_singleton(
            "frm2550qv2024:calendarNo1",
            RawValue::Text("true".to_string()),
        );
        let partial_errors = partial
            .try_to_bir_xml_payload()
            .expect_err("a partial radio group must not be completed from typed state");
        assert!(
            partial_errors
                .iter()
                .any(|(field, _)| field == "raw_editor.frm2550qv2024:fiscalNo1")
        );
    }

    #[test]
    fn checked_xml_rejects_unknown_and_orphan_raw_bindings_read_only() {
        let mut unknown_singleton = reviewed_sample_draft();
        unknown_singleton.raw_editor_state.set_singleton(
            "future_unreviewed_singleton",
            RawValue::Text("x".to_string()),
        );
        let singleton_errors = unknown_singleton
            .try_to_bir_xml_payload()
            .expect_err("unknown singleton raw key blocks checked XML");
        assert!(singleton_errors.iter().any(|(field, message)| {
            field == "raw_editor_state.bindings"
                && message.contains("unbound singleton field future_unreviewed_singleton")
        }));

        let mut unknown_repeated = reviewed_sample_draft();
        let existing_id = unknown_repeated.schedule_3[0]
            .instance_id
            .clone()
            .expect("reviewed draft row has identity");
        unknown_repeated.raw_editor_state.set_repeated(
            super::super::form_2550q::Form2550QRowFamily::Schedule3,
            existing_id,
            "future_unreviewed_member",
            RawValue::Text("x".to_string()),
        );
        let repeated_errors = unknown_repeated
            .try_to_bir_xml_payload()
            .expect_err("unknown repeated raw key blocks checked XML");
        assert!(repeated_errors.iter().any(|(field, message)| {
            field == "raw_editor_state.bindings"
                && message.contains("unbound Schedule 3 field future_unreviewed_member")
        }));

        let mut orphan = reviewed_sample_draft();
        let original_ids = orphan
            .schedule_1
            .iter()
            .map(|row| row.instance_id.clone())
            .collect::<Vec<_>>();
        let original_next_ordinal = orphan.row_identity_state.next_ordinal();
        orphan.raw_editor_state.set_repeated(
            super::super::form_2550q::Form2550QRowFamily::Schedule1,
            bir_rules::StableInstanceId::parse("row-00000000000000000999")
                .expect("strict orphan row ID"),
            "txtInputTax1",
            RawValue::Text("10.00".to_string()),
        );
        let orphan_errors = orphan
            .try_to_bir_xml_payload()
            .expect_err("orphan repeated raw row blocks checked XML");
        assert!(orphan_errors.iter().any(|(field, message)| {
            field == "raw_editor_state.bindings"
                && message.contains("orphan Schedule 1 row row-00000000000000000999")
        }));
        assert_eq!(
            orphan
                .schedule_1
                .iter()
                .map(|row| row.instance_id.clone())
                .collect::<Vec<_>>(),
            original_ids
        );
        assert_eq!(
            orphan.row_identity_state.next_ordinal(),
            original_next_ordinal
        );
    }

    #[test]
    fn reviewed_import_seeds_all_one_hundred_six_live_v1_raw_buffers() {
        let fields = reviewed_sample_draft().to_bir_field_map();
        let mut imported =
            Form2550QDraft::from_bir_field_map(&fields).expect("reviewed map imports");
        let targets = imported.reviewed_raw_binding_targets();
        assert_eq!(targets.len(), 106);
        let expected = targets
            .into_iter()
            .map(|target| {
                (
                    target.stable_path,
                    RawValue::Text(
                        fields
                            .get(&target.xml_key)
                            .unwrap_or_else(|| {
                                panic!("reviewed source is missing {}", target.xml_key)
                            })
                            .clone(),
                    ),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let actual_singletons = imported
            .raw_editor_state
            .singleton_fields()
            .iter()
            .map(|(field_key, value)| (field_key.clone(), value.clone()));
        let actual_repeated =
            imported
                .raw_editor_state
                .repeated_fields()
                .iter()
                .flat_map(|(family, instances)| {
                    instances.iter().flat_map(move |(instance_id, values)| {
                        values.iter().map(move |(member_key, value)| {
                            (
                                format!("{}/{instance_id}/{member_key}", raw_family_path(*family)),
                                value.clone(),
                            )
                        })
                    })
                });
        let actual = actual_singletons
            .chain(actual_repeated)
            .collect::<BTreeMap<_, _>>();
        assert_eq!(
            actual, expected,
            "all 106 reviewed XML source buffers must map exactly once to their stable raw path"
        );

        let source = Form2550QFieldValueSource::try_capture(&mut imported)
            .expect("imported raw state captures");
        assert_eq!(source.snapshot().fields().len(), 106);
        assert_eq!(source.snapshot().repeated_group_instances().len(), 6);
        assert_eq!(
            source
                .gaps()
                .iter()
                .filter(|gap| matches!(gap, Form2550QCaptureGap::MissingRawBuffer { .. }))
                .count(),
            0
        );
        assert_eq!(source.excluded_local_print_buffer_count(), 0);
    }

    #[test]
    fn encrypted_final_flag_and_standalone_filed_date_are_accepted() {
        let mut fields = reviewed_sample_draft().to_bir_field_map();
        fields.insert("txtFinalFlag".to_string(), "0".to_string());
        let mut xml = crate::bir_xml::generate_bir_xml(&fields);
        let date_div = "<div id=\"dateFiled\">dateFiled=2026%2F05%2F08dateFiled=</div>\n";
        xml = xml.replace(date_div, "<dateFiled>2026/05/08</dateFiled>");
        let draft = Form2550QDraft::from_bir_xml_payload(&xml).expect("encrypted shape imports");
        assert_eq!(draft.xml_final_flag, Form2550QXmlFinalFlag::Zero);
        assert_eq!(
            draft.date_filed,
            Some(Form2550QDate::new(2026, 5, 8).expect("date"))
        );
    }

    #[test]
    fn contradictory_quarter_flags_fail_closed() {
        let mut fields = reviewed_sample_draft().to_bir_field_map();
        fields.insert("frm2550qv2024:OptQuarter2".to_string(), "true".to_string());
        let errors = Form2550QDraft::from_bir_field_map(&fields).expect_err("must reject");
        assert!(errors.iter().any(|(field, _)| field == "quarter"));
    }

    #[test]
    fn source_computed_mismatch_is_rejected() {
        let mut fields = reviewed_sample_draft().to_bir_field_map();
        fields.insert(
            "frm2550qv2024:totalCurPurchase".to_string(),
            "1.00".to_string(),
        );
        let errors = Form2550QDraft::from_bir_field_map(&fields).expect_err("must reject");
        assert!(
            errors
                .iter()
                .any(|(field, _)| field == "frm2550qv2024:totalCurPurchase")
        );
    }

    #[test]
    fn malformed_money_is_rejected_instead_of_becoming_zero() {
        let mut fields = reviewed_sample_draft().to_bir_field_map();
        fields.insert(
            "frm2550qv2024:vatableSales".to_string(),
            "not-money".to_string(),
        );
        let errors = Form2550QDraft::from_bir_field_map(&fields).expect_err("must reject");
        assert!(
            errors
                .iter()
                .any(|(field, _)| field == "frm2550qv2024:vatableSales")
        );
    }
}

/// Schedule 1 modal row controls (`addlistSched1`), in DOM order. A row's
/// id is the member followed by its index: `txtDatePurchase1` + `0`.
const SCHEDULE_1_MODAL_MEMBERS: [&str; 9] = [
    "txtDatePurchase1",
    "txtSourceCode1",
    "txtDescription1",
    "txtAmountPurchase1",
    "txtInputTax1",
    "txtEstimatedLife1",
    "txtRecognizedLife1",
    "txtAllowedInputTax1",
    "txtBalanceInputTax1",
];
/// Schedule 3 modal row controls (`addlistSched3`).
const SCHEDULE_3_MODAL_MEMBERS: [&str; 5] = [
    "txtDateCovered3",
    "txtDateCovered3To",
    "txtNameWithHoldingAgent3",
    "txtIncomePayment3",
    "txtTotalTaxWithHeld3",
];
/// Schedule 4 modal row controls (`addlistSched4`).
const SCHEDULE_4_MODAL_MEMBERS: [&str; 6] = [
    "txtDate4",
    "txtDate4To",
    "txtNameOfMiller4",
    "txtNameOfTaxpayer4",
    "txtOfficialReceiptNumber4",
    "txtAmountPaid4",
];

/// One schedule's official "Add" list as the page saves it.
///
/// Opening a schedule's modal (`showNewSched1/3/4`) empties the two disabled
/// rows of the main table, and `addlistSched1/3/4` builds the rows inside the
/// modal, ahead of its total box. The page has no row limit. A schedule with
/// rows is therefore written as its modal rows `<member><index>` (from 0) just
/// before the anchor, and its two static rows are gone; an unused schedule
/// keeps the two static rows at their page defaults.
pub(crate) struct Form2550QModalSchedule {
    pub members: &'static [&'static str],
    /// The modal's first total box; the rows sit right before it.
    pub anchor: &'static str,
    /// Each used row's values, in member order.
    pub rows: Vec<Vec<String>>,
}

fn money_text(value: Option<f64>) -> String {
    value.map(format_money).unwrap_or_default()
}

impl Form2550QDraft {
    /// Schedules 1, 3 and 4 that hold rows, as the official modals save them.
    pub(crate) fn official_modal_schedules(&self) -> Vec<Form2550QModalSchedule> {
        let date = |value: Option<Form2550QDate>| value.map(|d| d.to_string()).unwrap_or_default();
        // `blockletterWithout2Decimal`: whole months.
        let life = |value: Option<u16>| value.map(|m| m.to_string()).unwrap_or_default();
        let mut schedules = Vec::new();
        let rows_1: Vec<Vec<String>> = self
            .schedule_1
            .iter()
            .filter(|row| !row.is_empty())
            .map(|row| {
                vec![
                    date(row.purchase_or_import_date),
                    row.source_code.clone(),
                    row.description.clone(),
                    money_text(row.purchase_or_import_amount),
                    money_text(row.input_tax),
                    life(row.estimated_life_months),
                    life(row.recognized_life_months),
                    money_text(row.allowable_input_tax_for_period),
                    money_text(row.balance_to_next_period),
                ]
            })
            .collect();
        if !rows_1.is_empty() {
            schedules.push(Form2550QModalSchedule {
                members: &SCHEDULE_1_MODAL_MEMBERS,
                anchor: "txtTotalAmountOfBalanceofInputTaxFromPrevious",
                rows: rows_1,
            });
        }
        let rows_3: Vec<Vec<String>> = self
            .schedule_3
            .iter()
            .filter(|row| !row.is_empty())
            .map(|row| {
                vec![
                    date(row.period_from),
                    date(row.period_to),
                    row.withholding_agent_name.clone(),
                    money_text(row.income_payment),
                    money_text(row.tax_withheld),
                ]
            })
            .collect();
        if !rows_3.is_empty() {
            schedules.push(Form2550QModalSchedule {
                members: &SCHEDULE_3_MODAL_MEMBERS,
                anchor: "txtTotalAmountofIncomePayment",
                rows: rows_3,
            });
        }
        let rows_4: Vec<Vec<String>> = self
            .schedule_4
            .iter()
            .filter(|row| !row.is_empty())
            .map(|row| {
                vec![
                    date(row.period_from),
                    date(row.period_to),
                    row.miller_name.clone(),
                    row.taxpayer_name.clone(),
                    row.official_receipt_number.clone(),
                    money_text(row.amount_paid),
                ]
            })
            .collect();
        if !rows_4.is_empty() {
            schedules.push(Form2550QModalSchedule {
                members: &SCHEDULE_4_MODAL_MEMBERS,
                anchor: "txtAmountPaidSched4",
                rows: rows_4,
            });
        }
        schedules
    }

    /// The controls `saveEncryptedProfile` walks, keyed by official element
    /// id: the reviewed map, with each used schedule's static rows replaced by
    /// its modal rows. The static rows are disabled on the official page, so
    /// they always keep the page defaults.
    pub(crate) fn official_control_map(&self) -> BTreeMap<String, String> {
        let mut fields = self.to_bir_field_map();
        fields.remove("dateFiled");
        for members in [
            &SCHEDULE_1_MODAL_MEMBERS[..],
            &SCHEDULE_3_MODAL_MEMBERS[..],
            &SCHEDULE_4_MODAL_MEMBERS[..],
        ] {
            for member in members {
                for index in 0..2 {
                    fields.remove(&format!("{member}{index}"));
                }
            }
        }
        for schedule in self.official_modal_schedules() {
            for (index, row) in schedule.rows.iter().enumerate() {
                for (member, value) in schedule.members.iter().zip(row) {
                    fields.insert(format!("{member}{index}"), value.clone());
                }
            }
        }
        for popup in self.official_specify_popups() {
            for (index, row) in popup.rows.iter().enumerate() {
                fields.insert(
                    format!(
                        "frm2550qv2024:totalTaxPayableNo{}Description{index}",
                        popup.item
                    ),
                    row.description.clone(),
                );
                fields.insert(
                    format!("frm2550qv2024:totalTaxPayableNo{}Amount{index}", popup.item),
                    money_text(row.amount),
                );
            }
            fields.insert(popup.subtotal_key(), money_text(popup.subtotal));
        }
        fields
    }

    /// Items 19, 42, 47 and 56 whose "(Add more...)" popup holds rows.
    pub(crate) fn official_specify_popups(&self) -> Vec<Form2550QSpecifyPopup<'_>> {
        [
            (
                19,
                &self.item_19_additional_rows,
                self.part_ii.item_19_other_credit_or_payment,
            ),
            (
                42,
                &self.item_42_additional_rows,
                self.part_iv.item_42b_other_input_tax,
            ),
            (
                47,
                &self.item_47_additional_rows,
                self.part_iv.item_47a_other_purchases,
            ),
            (
                56,
                &self.item_56_additional_rows,
                self.part_iv.item_56b_other_deduction,
            ),
        ]
        .into_iter()
        .filter(|(_, rows, _)| !rows.is_empty())
        .map(|(item, rows, subtotal)| Form2550QSpecifyPopup {
            item,
            rows,
            subtotal,
        })
        .collect()
    }

    /// The generated layout with each used schedule's two static rows taken
    /// out and its modal rows spliced in before the modal's total box, where
    /// `addlistSched1/3/4` puts them.
    pub fn official_layout(&self) -> Result<crate::official_xml::OfficialLayout, String> {
        use crate::official_xml::{Entry, Part};
        let base = crate::official_xml::layout(super::form_2550q::FORM_2550Q_LAYOUT_ID)
            .map_err(|e| e.to_string())?;
        let key_of = |entry: &Entry| match entry {
            Entry::Bool { key, .. } | Entry::Value { key, .. } => key.clone(),
        };
        let mut layout = base.clone();
        for schedule in self.official_modal_schedules() {
            let mut templates = Vec::new();
            for member in schedule.members {
                let key = format!("{member}0");
                let template = layout
                    .entries
                    .iter()
                    .find(|entry| key_of(entry) == key)
                    .cloned()
                    .ok_or_else(|| format!("no template entry {key}"))?;
                templates.push((key, template));
            }
            let statics: Vec<String> = schedule
                .members
                .iter()
                .flat_map(|member| (0..2).map(move |index| format!("{member}{index}")))
                .collect();
            layout
                .entries
                .retain(|entry| !statics.contains(&key_of(entry)));
            let at = layout
                .entries
                .iter()
                .position(|entry| key_of(entry) == schedule.anchor)
                .ok_or_else(|| format!("no anchor entry {}", schedule.anchor))?;
            let mut rows = Vec::new();
            for index in 0..schedule.rows.len() {
                for (member, (template_key, template)) in schedule.members.iter().zip(&templates) {
                    let new_key = format!("{member}{index}");
                    rows.push(match template.clone() {
                        Entry::Bool { default, after, .. } => Entry::Bool {
                            key: new_key,
                            default,
                            after,
                        },
                        Entry::Value {
                            parts,
                            default,
                            number,
                            after,
                            ..
                        } => Entry::Value {
                            key: new_key.clone(),
                            number,
                            parts: parts
                                .into_iter()
                                .map(|part| match part {
                                    Part::Source {
                                        source,
                                        codec,
                                        uppercase,
                                    } if source == *template_key => Part::Source {
                                        source: new_key.clone(),
                                        codec,
                                        uppercase,
                                    },
                                    other => other,
                                })
                                .collect(),
                            default,
                            after,
                        },
                    });
                }
            }
            layout.entries.splice(at..at, rows);
        }
        for popup in self.official_specify_popups() {
            let anchor = popup.subtotal_key();
            let at = layout
                .entries
                .iter()
                .position(|entry| key_of(entry) == anchor)
                .ok_or_else(|| format!("no anchor entry {anchor}"))?;
            let after = match &layout.entries[at] {
                Entry::Bool { after, .. } | Entry::Value { after, .. } => after.clone(),
            };
            let mut rows = Vec::new();
            for index in 0..popup.rows.len() {
                for (member, default) in [("Description", ""), ("Amount", "0.00")] {
                    let key = format!(
                        "frm2550qv2024:totalTaxPayableNo{}{member}{index}",
                        popup.item
                    );
                    rows.push(Entry::Value {
                        parts: vec![Part::Source {
                            source: key.clone(),
                            codec: crate::official_xml::Codec::Raw,
                            uppercase: false,
                        }],
                        key,
                        default: default.to_string(),
                        number: None,
                        after: after.clone(),
                    });
                }
            }
            layout.entries.splice(at..at, rows);
        }
        Ok(layout)
    }
}

/// One item's "(Add more...)" popup as `saveEncryptedProfile` walks it.
///
/// `showModalNo19/42/47/56` and `addColNo19/42/47/56` build the rows inside
/// the item's modal, ahead of its subtotal box `resultOtherCreditsNo<item>`,
/// as `frm2550qv2024:totalTaxPayableNo<item>Description<i>` and `...Amount<i>`
/// (from 0). The rows stay in the page after the popup closes, so they are
/// written right before the subtotal, and the subtotal holds the rows' total.
/// The static line keeps its place (`VARIOUS` and the total).
pub(crate) struct Form2550QSpecifyPopup<'a> {
    pub item: u8,
    pub rows: &'a [super::form_2550q::Form2550QAdditionalItemRow],
    pub subtotal: Option<f64>,
}

impl Form2550QSpecifyPopup<'_> {
    fn subtotal_key(&self) -> String {
        format!("resultOtherCreditsNo{}", self.item)
    }
}
