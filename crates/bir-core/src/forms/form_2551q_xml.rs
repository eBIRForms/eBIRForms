use super::form_2551q::{Form2551QDraft, Item13Election, OverpaymentDisposition, TaxPeriodBasis};
use super::{AtcRateResolution, resolve_2551q_atc_rate};
use std::collections::BTreeMap;
use std::sync::OnceLock;

/// The official form id of this rule package and layout.
pub const FORM_2551Q_FORM_ID: &str = "2551q-v2018";

#[derive(serde::Deserialize)]
struct OfficialAtcOptions {
    options: Vec<OfficialAtcOption>,
}

/// One option of the official `drpATC` dropdown.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct OfficialAtcOption {
    /// The option value the official form submits (1-based list position).
    pub value: String,
    pub code: String,
    /// The rate text the official form submits in `txtATCRate`, e.g. `3.0`.
    pub rate: String,
}

/// The official 2551Q ATC option for `code` at `rate` (a fraction, e.g.
/// 0.03). PT010 has two options: 3% (value 1) and the 1% of July 2020 to
/// June 2023 (value 23). `None` when the official dropdown has no such entry.
pub fn official_atc_option(code: &str, rate: f64) -> Option<&'static OfficialAtcOption> {
    static OPTIONS: OnceLock<Vec<OfficialAtcOption>> = OnceLock::new();
    OPTIONS
        .get_or_init(|| {
            serde_json::from_str::<OfficialAtcOptions>(include_str!(
                "../../data/official-xml/2551q-v2018-atc-options.json"
            ))
            .expect("valid 2551Q ATC options")
            .options
        })
        .iter()
        .find(|option| {
            option.code == code
                && option
                    .rate
                    .parse::<f64>()
                    .is_ok_and(|percent| (percent / 100.0 - rate).abs() < 1e-9)
        })
}

impl Form2551QDraft {
    /// Build the official eBIRForms field set: official control ids and the
    /// values the official controls hold. Controls left out keep the official
    /// page default when [`Self::to_bir_xml_payload`] writes the plaintext.
    ///
    /// This low-level helper exposes exactly six Schedule 1 slots. User-facing
    /// export must call [`Self::to_bir_xml_payload`], which validates the draft
    /// first and rejects additional saved/printable rows instead of truncating
    /// them into a seemingly fileable payload.
    pub fn to_bir_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = BTreeMap::new();
        let (tin1, tin2, tin3, branch) = split_tin(&self.tin);

        // Legacy / UI Fields from original eBIRForms
        insert(&mut fields, "driveSelectTPExport", "0");
        insert(&mut fields, "ebirOnlineConfirmUsername", "");
        insert(&mut fields, "ebirOnlineSecret", "");
        insert(&mut fields, "ebirOnlineUsername", "");
        insert(&mut fields, "txtEnroll", "Y");
        insert(&mut fields, "txtFinalFlag", "1");
        insert(&mut fields, "txtDateExpiry", "");
        insert(&mut fields, "txtTaxAgentNo", "");

        insert_bool(
            &mut fields,
            "frm2551Qv2018:forThe_1",
            matches!(self.tax_period_basis, TaxPeriodBasis::Calendar),
        );
        insert_bool(
            &mut fields,
            "frm2551Qv2018:forThe_2",
            matches!(self.tax_period_basis, TaxPeriodBasis::Fiscal),
        );
        // The official select's option values are two digits: "01".."12".
        insert(
            &mut fields,
            "frm2551Qv2018:rtnMonth",
            format!("{:02}", self.year_end_month),
        );
        insert(
            &mut fields,
            "frm2551Qv2018:txtYear",
            self.taxable_year.to_string(),
        );
        for i in 1..=4 {
            insert_bool(
                &mut fields,
                &format!("frm2551Qv2018:qtr_{}", i),
                self.quarter == i,
            );
        }

        insert_bool(&mut fields, "frm2551Qv2018:amendedRtn_1", self.is_amended);
        insert_bool(&mut fields, "frm2551Qv2018:amendedRtn_2", !self.is_amended);
        insert_bool(&mut fields, "frm2551Qv2018:taxTreaty_1", self.tax_relief);
        insert_bool(&mut fields, "frm2551Qv2018:taxTreaty_2", !self.tax_relief);
        insert(
            &mut fields,
            "frm2551Qv2018:txtTaxReliefSpecify",
            // Official options: 0 (blank, the page default), 1 Special
            // Rate, 2 International Tax Treaty. `validate` rejects anything
            // else when relief is Yes.
            if self.tax_relief {
                crate::validation::official_tax_relief_code(&self.tax_relief_specification, false)
                    .unwrap_or_else(|| self.tax_relief_specification.trim().to_string())
            } else {
                "0".to_string()
            },
        );
        insert_bool(
            &mut fields,
            "frm2551Qv2018:taxRate1",
            matches!(self.item_13_election, Item13Election::Graduated),
        );
        insert_bool(
            &mut fields,
            "frm2551Qv2018:taxRate2",
            matches!(self.item_13_election, Item13Election::EightPercent),
        );
        insert(
            &mut fields,
            "frm2551Qv2018:txtSheets",
            self.number_of_attached_sheets.to_string(),
        );

        insert(&mut fields, "frm2551Qv2018:txtTIN1", tin1.clone());
        insert(&mut fields, "frm2551Qv2018:txtTIN2", tin2.clone());
        insert(&mut fields, "frm2551Qv2018:txtTIN3", tin3.clone());
        insert(&mut fields, "frm2551Qv2018:txtBranchCode", branch.clone());
        insert(
            &mut fields,
            "frm2551Qv2018:txtRDOCode",
            self.rdo_code.clone(),
        );
        // Name and address are uppercased to mirror the official `capital()`
        // onblur handler. Profile: official-match. The email is not: the
        // effective `capital()` (string-util.js L236) skips `txtEmail`, so it
        // is submitted as entered and only printed in capitals by the renderer.
        insert(
            &mut fields,
            "frm2551Qv2018:registeredName",
            self.taxpayer_name.to_uppercase(),
        );
        insert(
            &mut fields,
            "frm2551Qv2018:registeredAddress",
            self.registered_address.to_uppercase(),
        );
        insert(&mut fields, "frm2551Qv2018:zipCode", self.zip_code.clone());
        insert(
            &mut fields,
            "frm2551Qv2018:telNo",
            self.contact_number.clone(),
        );
        insert(&mut fields, "txtEmail", self.email.clone());

        insert_money(&mut fields, "frm2551Qv2018:txt14", self.total_tax_due);
        insert_money(
            &mut fields,
            "frm2551Qv2018:txt15",
            self.creditable_tax_withheld,
        );
        insert_money(
            &mut fields,
            "frm2551Qv2018:txt16",
            if self.is_amended {
                self.tax_paid_previous
            } else {
                0.0
            },
        );
        insert(
            &mut fields,
            "frm2551Qv2018:txt17Specify",
            self.other_tax_credit_description.clone(),
        );
        insert_money(&mut fields, "frm2551Qv2018:txt17", self.other_tax_credit);
        insert_money(&mut fields, "frm2551Qv2018:txt18", self.total_tax_credits);
        insert_money(&mut fields, "frm2551Qv2018:txt19", self.tax_payable);
        insert_money(&mut fields, "frm2551Qv2018:txt20", self.surcharge);
        insert_money(&mut fields, "frm2551Qv2018:txt21", self.interest);
        insert_money(&mut fields, "frm2551Qv2018:txt22", self.compromise);
        insert_money(&mut fields, "frm2551Qv2018:txt23", self.total_penalties);
        insert_money(
            &mut fields,
            "frm2551Qv2018:txt24",
            self.total_amount_payable,
        );
        insert_money(&mut fields, "txtTotalSched1", self.total_tax_due);
        insert_bool(
            &mut fields,
            "frm2551Qv2018:overPayment1",
            matches!(self.overpayment_disposition, OverpaymentDisposition::Refund),
        );
        insert_bool(
            &mut fields,
            "frm2551Qv2018:overPayment2",
            matches!(
                self.overpayment_disposition,
                OverpaymentDisposition::TaxCreditCertificate
            ),
        );

        for i in 0..6 {
            let row = self.schedule_1.get(i);
            // The official dropdown submits its option value (a list
            // position) and the list's rate text, not the ATC code. A row
            // with no official option (unknown ATC, split period) keeps the
            // blank option; `validate` rejects such a draft.
            let option = row.and_then(|r| {
                let rate = match resolve_2551q_atc_rate(
                    r.atc.trim(),
                    self.taxable_year,
                    self.quarter,
                    self.year_end_month,
                ) {
                    Some(AtcRateResolution::Single(rate)) => rate,
                    _ => return None,
                };
                official_atc_option(r.atc.trim(), rate)
            });
            let atc = option.map_or("0", |option| option.value.as_str());
            let atc_rate = option.map_or("0.00", |option| option.rate.as_str());
            let atc_amt = row
                .map(|r| format_money(r.taxable_amount))
                .unwrap_or_else(|| "0.00".to_string());
            let atc_due = row
                .map(|r| format_money(r.tax_due))
                .unwrap_or_else(|| "0.00".to_string());

            insert(&mut fields, &format!("drpATC{}", i + 1), atc.to_string());
            insert(&mut fields, &format!("txtATCAmt{}", i + 1), atc_amt);
            insert(&mut fields, &format!("txtATCRate{}", i + 1), atc_rate);
            insert(&mut fields, &format!("txtATCDue{}", i + 1), atc_due);
        }

        insert(&mut fields, "frm2551Qv2018:txtPg2TIN1", tin1);
        insert(&mut fields, "frm2551Qv2018:txtPg2TIN2", tin2);
        insert(&mut fields, "frm2551Qv2018:txtPg2TIN3", tin3);
        insert(&mut fields, "frm2551Qv2018:txtPg2BranchCode", branch);
        insert(
            &mut fields,
            "frm2551Qv2018:txtPg2TaxpayerName",
            self.taxpayer_name.to_uppercase(),
        );
        insert(&mut fields, "frm2551Qv2018:txtCurrentPage", "1");
        insert(&mut fields, "frm2551Qv2018:txtMaxPage", "2");

        // This field is legal tax-agent metadata, not the XML render time.
        // Keep it blank until the draft owns an explicit issue date.
        insert(&mut fields, "txtDateIssue", "");

        fields
    }

    pub fn to_bir_xml_payload(&self) -> Result<String, Vec<(String, String)>> {
        let errors = <Self as super::FormValidator>::validate(self);
        if errors.is_empty() {
            let layout = crate::official_xml::layout(FORM_2551Q_FORM_ID)
                .map_err(|error| vec![("xml".to_string(), error.to_string())])?;
            crate::official_xml::write(layout, &self.to_bir_field_map())
                .map_err(|error| vec![("xml".to_string(), error.to_string())])
        } else {
            Err(errors)
        }
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

fn insert_money(map: &mut BTreeMap<String, String>, key: &str, value: f64) {
    insert(map, key, format_money(value));
}

/// Official money text: `1,234.50` (see [`crate::official_xml::official_amount`]).
fn format_money(value: f64) -> String {
    crate::official_xml::official_amount(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forms::ATC_TABLE_2551Q;
    use crate::forms::form_2551q::Schedule1Row;
    use crate::naming::Tin;
    use crate::profile::{TaxpayerProfile, TaxpayerType};

    fn sample_draft() -> Form2551QDraft {
        let profile = TaxpayerProfile {
            id: None,
            full_name: "Juana Santos".into(),
            tin: Tin {
                segment1: "000".into(),
                segment2: "123".into(),
                segment3: "453".into(),
                branch: "000".into(),
            },
            rdo_code: "018".into(),
            line_of_business: "Software".into(),
            registered_address: "Sample Brgy".into(),
            zip_code: "2200".into(),
            phone: "09170000100".into(),
            email: "tax@example.com".into(),
            default_form_type: "2551Qv2018".into(),
            taxpayer_type: TaxpayerType::Individual,
            is_vat_registered: false,
            business_start_date: chrono::NaiveDate::from_ymd_opt(2010, 1, 1),
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
        let mut draft = Form2551QDraft::new_from_profile(&profile, 2026, 1);
        draft.tin = "000123453000".to_string();
        draft.item_13_election = Item13Election::Graduated;
        draft.schedule_1[0].taxable_amount = 10_000.0;
        draft.creditable_tax_withheld = 12.5;
        draft.recompute(None);
        draft
    }

    #[test]
    fn field_map_contains_required_print_and_xml_keys() {
        let fields = sample_draft().to_bir_field_map();

        for key in [
            "frm2551Qv2018:txtYear",
            "frm2551Qv2018:txtTIN1",
            "frm2551Qv2018:txtTIN2",
            "frm2551Qv2018:txtTIN3",
            "frm2551Qv2018:txtBranchCode",
            "frm2551Qv2018:txt14",
            "frm2551Qv2018:txt18",
            "frm2551Qv2018:txt19",
            "frm2551Qv2018:txt20",
            "frm2551Qv2018:txt21",
            "frm2551Qv2018:txt22",
            "frm2551Qv2018:txt23",
            "frm2551Qv2018:txt24",
            "txtTotalSched1",
            "frm2551Qv2018:txtPg2TIN1",
            "frm2551Qv2018:txtPg2TaxpayerName",
        ] {
            assert!(fields.contains_key(key), "missing {key}");
        }

        assert_eq!(fields["frm2551Qv2018:txtTIN1"], "000");
        assert_eq!(fields["frm2551Qv2018:txtTIN2"], "123");
        assert_eq!(fields["frm2551Qv2018:txtTIN3"], "453");
        assert_eq!(fields["frm2551Qv2018:txtBranchCode"], "00000");
        assert_eq!(fields["frm2551Qv2018:forThe_1"], "true");
        assert_eq!(fields["frm2551Qv2018:forThe_2"], "false");
        assert_eq!(fields["frm2551Qv2018:rtnMonth"], "12");
        assert!(!fields.contains_key("__year_ended"));
        assert_eq!(fields["frm2551Qv2018:txtSheets"], "0");

        // This Individual Q1 PT010 return explicitly selects the graduated
        // election; no overpayment disposition is selected for a positive balance.
        assert_eq!(fields["frm2551Qv2018:taxRate1"], "true");
        assert_eq!(fields["frm2551Qv2018:taxRate2"], "false");
        assert_eq!(fields["frm2551Qv2018:overPayment1"], "false");
        assert_eq!(fields["frm2551Qv2018:overPayment2"], "false");
        assert_eq!(fields["txtDateIssue"], "");
    }

    #[test]
    fn writer_keys_are_all_official_controls() {
        // Every key must be a control the official saveXMLsubmit() writes.
        let layout = crate::official_xml::layout(FORM_2551Q_FORM_ID).expect("layout");
        let official = layout.keys();
        for key in sample_draft().to_bir_field_map().keys() {
            assert!(
                official.contains(key.as_str()),
                "{key} is not an official control"
            );
        }
    }

    #[test]
    fn writer_frm_keys_are_in_fields_json() {
        let inventory: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/rules-forms/2551q-v2018/fields.json"
        ))
        .expect("2551Q fields inventory");
        let serialized: std::collections::BTreeSet<String> = inventory["fields"]
            .as_array()
            .expect("fields array")
            .iter()
            .filter_map(|row| row["serialized_key"].as_str().map(str::to_string))
            .collect();

        let frm_keys: std::collections::BTreeSet<String> = sample_draft()
            .to_bir_field_map()
            .into_keys()
            .filter(|key| key.starts_with("frm2551Qv2018:"))
            .collect();

        let missing: std::collections::BTreeSet<String> =
            frm_keys.difference(&serialized).cloned().collect();
        assert!(
            missing.is_empty(),
            "writer frm2551Qv2018:* keys must stay inside fields.json: {missing:?}"
        );

        for key in [
            "frm2551Qv2018:txtTIN1",
            "frm2551Qv2018:txtTIN2",
            "frm2551Qv2018:txtTIN3",
            "frm2551Qv2018:txtBranchCode",
        ] {
            assert!(
                serialized.contains(key),
                "TIN key {key} missing from fields.json"
            );
            assert!(
                frm_keys.contains(key),
                "TIN key {key} missing from writer map"
            );
        }
    }

    #[test]
    fn field_map_serializes_explicit_filing_and_legal_values() {
        let mut draft = sample_draft();
        draft.tax_period_basis = TaxPeriodBasis::Fiscal;
        draft.year_end_month = 6;
        draft.number_of_attached_sheets = 3;
        draft.tax_relief = true;
        draft.tax_relief_specification = "International Tax Treaty".to_string();
        draft.item_13_election = Item13Election::EightPercent;
        draft.other_tax_credit_description = "Prior quarter adjustment".to_string();
        draft.overpayment_disposition = OverpaymentDisposition::Refund;

        let fields = draft.to_bir_field_map();

        assert_eq!(fields["frm2551Qv2018:forThe_1"], "false");
        assert_eq!(fields["frm2551Qv2018:forThe_2"], "true");
        // The official select's option values are two digits.
        assert_eq!(fields["frm2551Qv2018:rtnMonth"], "06");
        assert_eq!(fields["frm2551Qv2018:txtSheets"], "3");
        assert_eq!(fields["frm2551Qv2018:taxTreaty_1"], "true");
        assert_eq!(fields["frm2551Qv2018:taxTreaty_2"], "false");
        assert_eq!(fields["frm2551Qv2018:txtTaxReliefSpecify"], "2");
        assert_eq!(fields["frm2551Qv2018:taxRate1"], "false");
        assert_eq!(fields["frm2551Qv2018:taxRate2"], "true");
        assert_eq!(
            fields["frm2551Qv2018:txt17Specify"],
            "Prior quarter adjustment"
        );
        assert_eq!(fields["frm2551Qv2018:overPayment1"], "true");
        assert_eq!(fields["frm2551Qv2018:overPayment2"], "false");
    }

    #[test]
    fn unmapped_tax_relief_specification_serializes_verbatim_not_blank() {
        let mut draft = sample_draft();
        draft.tax_relief = true;
        draft.tax_relief_specification = "  Special law 123 ".to_string();

        let fields = draft.to_bir_field_map();

        assert_eq!(
            fields["frm2551Qv2018:txtTaxReliefSpecify"],
            "Special law 123"
        );
    }

    #[test]
    fn field_map_maps_each_single_choice_without_overlap() {
        let mut draft = sample_draft();
        draft.item_13_election = Item13Election::Graduated;
        draft.overpayment_disposition = OverpaymentDisposition::TaxCreditCertificate;

        let fields = draft.to_bir_field_map();

        assert_eq!(fields["frm2551Qv2018:taxRate1"], "true");
        assert_eq!(fields["frm2551Qv2018:taxRate2"], "false");
        assert_eq!(fields["frm2551Qv2018:overPayment1"], "false");
        assert_eq!(fields["frm2551Qv2018:overPayment2"], "true");
    }

    #[test]
    fn field_map_never_prints_item_16_on_a_non_amended_return() {
        let mut draft = sample_draft();
        draft.is_amended = false;
        draft.tax_paid_previous = 500.0;
        assert_eq!(draft.to_bir_field_map()["frm2551Qv2018:txt16"], "0.00");

        draft.is_amended = true;
        assert_eq!(draft.to_bir_field_map()["frm2551Qv2018:txt16"], "500.00");
    }

    #[test]
    fn xml_export_uses_canonical_field_map() {
        let xml = sample_draft()
            .to_bir_xml_payload()
            .expect("the valid sample draft should serialize");

        assert!(xml.contains("frm2551Qv2018:txt18="));
        assert!(xml.contains("frm2551Qv2018:txtPg2TaxpayerName="));
        assert!(xml.contains("txtTotalSched1="));
    }

    #[test]
    fn xml_export_fails_closed_before_ten_printable_rows_can_be_truncated() {
        let mut draft = sample_draft();
        draft.schedule_1 = [
            "PT010", "PT040", "PT041", "PT060", "PT070", "PT090", "PT140", "PT150", "PT160",
            "PT170",
        ]
        .into_iter()
        .map(|code| Schedule1Row::new(code).expect("test ATC must be canonical"))
        .collect();
        draft.ensure_required_schedule_attachment_sheets();
        draft.recompute(None);

        let errors = draft
            .to_bir_xml_payload()
            .expect_err("the reviewed XML has no fields beyond Schedule 1 row 6");

        assert!(errors.iter().any(|(field, message)| {
            field == "schedule_1"
                && message.contains("at most 6")
                && message.contains("official XML attachment protocol")
        }));
        assert_eq!(draft.schedule_1.len(), 10);
        assert_eq!(draft.number_of_attached_sheets, 1);
    }

    #[test]
    fn xml_export_rejects_stale_or_tampered_derived_amounts() {
        for field in [
            "total_tax_due",
            "total_tax_credits",
            "tax_payable",
            "total_penalties",
            "total_amount_payable",
        ] {
            let mut draft = sample_draft();
            match field {
                "total_tax_due" => draft.total_tax_due += 1.0,
                "total_tax_credits" => draft.total_tax_credits += 1.0,
                "tax_payable" => draft.tax_payable += 1.0,
                "total_penalties" => draft.total_penalties += 1.0,
                "total_amount_payable" => draft.total_amount_payable += 1.0,
                _ => unreachable!(),
            }

            let errors = draft
                .to_bir_xml_payload()
                .expect_err("inconsistent derived amounts must not produce XML");
            assert!(
                errors.iter().any(|(error_field, _)| error_field == field),
                "expected {field} invariant error; got {errors:?}"
            );
        }
    }

    #[test]
    fn field_map_emits_the_registry_owned_atc_rate() {
        let mut draft = sample_draft();
        draft.schedule_1[0].tax_rate = 0.030_000_000_000_5;

        let fields = draft.to_bir_field_map();
        // Official option 1 is PT010 at the list's rate text "3.0".
        assert_eq!(fields["drpATC1"], "1");
        assert_eq!(fields["txtATCRate1"], "3.0");
        draft
            .to_bir_xml_payload()
            .expect("a tolerated binary float delta must serialize canonically");
    }

    #[test]
    fn every_registry_rate_serializes_as_a_canonical_decimal() {
        for entry in ATC_TABLE_2551Q {
            let mut draft = sample_draft();
            let mut row = Schedule1Row::new(entry.code).expect("registry entry must resolve");
            row.taxable_amount = 10_000.0;
            draft.schedule_1 = vec![row];
            draft.item_13_election = if entry.code == "PT010" {
                Item13Election::Graduated
            } else {
                Item13Election::NotApplicable
            };
            draft.recompute(None);

            let fields = draft.to_bir_field_map();
            let option = official_atc_option(entry.code, entry.rate)
                .unwrap_or_else(|| panic!("{} has no official option", entry.code));
            assert_eq!(fields["drpATC1"], option.value, "{}", entry.code);
            assert_eq!(
                fields["txtATCRate1"],
                format!("{:.1}", entry.rate * 100.0),
                "{} must use the official rate text",
                entry.code
            );
            draft.to_bir_xml_payload().unwrap_or_else(|errors| {
                panic!("{} failed XML validation: {errors:?}", entry.code)
            });
        }
    }

    #[test]
    fn field_map_emits_the_period_owned_temporary_pt010_rate() {
        let mut draft = sample_draft();
        draft.taxable_year = 2021;
        draft.quarter = 3;
        draft.annual_income_tax_election =
            Some(crate::forms::form_2551q::AnnualIncomeTaxElection::Graduated);
        draft.item_13_election = Item13Election::NotApplicable;
        draft.recompute(None);

        assert_eq!(draft.schedule_1[0].tax_rate, 0.01);
        let fields = draft.to_bir_field_map();
        // The 1% PT010 is a separate official option, value 23.
        assert_eq!(fields["drpATC1"], "23");
        assert_eq!(fields["txtATCRate1"], "1.0");
        draft
            .to_bir_xml_payload()
            .expect("the temporary statutory rate must serialize canonically");
    }

    #[test]
    fn xml_export_enforces_item_13_applicability_before_serializing_boxes() {
        let mut draft = sample_draft();
        draft.quarter = 2;

        let errors = draft
            .to_bir_xml_payload()
            .expect_err("a later-quarter graduated election must not serialize");
        assert!(errors.iter().any(|(field, _)| field == "item_13_election"));

        draft.annual_income_tax_election =
            Some(crate::forms::form_2551q::AnnualIncomeTaxElection::Graduated);
        draft.item_13_election = Item13Election::NotApplicable;
        let fields = draft.to_bir_field_map();
        assert_eq!(fields["frm2551Qv2018:taxRate1"], "false");
        assert_eq!(fields["frm2551Qv2018:taxRate2"], "false");
        draft.recompute(None);
        draft
            .to_bir_xml_payload()
            .expect("a later-quarter NotApplicable election should serialize");
    }

    #[test]
    fn xml_export_rejects_negative_or_non_finite_manual_penalties() {
        let cases = [
            ("surcharge", -1.0),
            ("interest", -1.0),
            ("compromise", -1.0),
            ("surcharge", f64::NAN),
            ("interest", f64::INFINITY),
            ("compromise", f64::NEG_INFINITY),
        ];

        for (field, value) in cases {
            let mut draft = sample_draft();
            draft.auto_compute_penalties = false;
            match field {
                "surcharge" => draft.surcharge = value,
                "interest" => draft.interest = value,
                "compromise" => draft.compromise = value,
                _ => unreachable!("test case uses a known penalty field"),
            }

            let errors = draft
                .to_bir_xml_payload()
                .expect_err("invalid manual penalties must not produce XML");

            assert!(
                errors.iter().any(|(error_field, _)| error_field == field),
                "expected {field} to be rejected for {value:?}; got {errors:?}"
            );
        }
    }

    #[test]
    fn xml_export_rejects_a_coherently_tampered_automatic_penalty_chain() {
        let mut draft = sample_draft();
        draft.surcharge += 100.0;
        draft.total_penalties += 100.0;
        draft.total_amount_payable += 100.0;

        let errors = draft
            .to_bir_xml_payload()
            .expect_err("automatic penalties must be recalculated at the XML boundary");
        assert!(errors.iter().any(|(field, message)| {
            field == "surcharge" && message.contains("automatic surcharge")
        }));
    }
}
