//! BIR Form 0619-E, January 2018 (ENCS).
//!
//! This model is intentionally limited to behavior corroborated by the locked
//! official form, the reviewed plain/encrypted 0619-E payload pair, and the
//! hash-locked eBIRForms 7.9.5.0 package sources. Rust semantically replays but
//! does not byte-for-byte reproduce the reviewed ciphertext, while the package
//! obtains current transport configuration at runtime and delegates
//! encryption/upload to omitted executables. Form 0619-E therefore remains
//! manual/external for submission.

use super::penalty_items::PenaltyState;
use super::queueable::SubmissionLifecycle;
use super::{FilingPeriod, FormValidator, TypedBirForm};
use crate::profile::TaxpayerProfile;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const FORM_CODE: &str = "0619E";
pub const FORM_REVISION: &str = "2018";
pub const FORM_TYPE_ID: &str = "0619Ev2018";
pub const ATC_CODE: &str = "WME10";
pub const TAX_TYPE_CODE: &str = "WE";
pub const QUEUE_SUBMISSION_SUPPORTED: bool = true;

/// Hash-locked source evidence for the exact January 2018 revision.
pub const OFFICIAL_FORM_SHA256: &str =
    "0418160d63d4e6f68c34f2bad553273a5d148c3686d8562d338d35fcdd0c5215";
pub const REVIEWED_EDITABLE_XML_SHA256: &str =
    "a6f21e372a1ce6d707ede13f2447290683ab302d859c3b684a06c55788cbfade";
pub const REVIEWED_ENCRYPTED_XML_SHA256: &str =
    "1c49950df1197906bb73ddbb5d0f5f5e1c3f488f376e05b6d53febc1b32016ab";
pub const REVIEWED_DECRYPTED_XML_SHA256: &str =
    "6a1911a359efedae7e35fa21c2def9af62c7cc1194e768d4bca5f3193c33fef4";
pub const EXACT_REVIEWED_PLAIN_XML_FIELD_COUNT: usize = 58;
pub const EXACT_REVIEWED_ENCRYPTED_XML_FIELD_COUNT: usize = 59;
pub const REVIEWED_ENCRYPTED_XML_EXTRA_FIELD: &str = "frm0619E:txtAddress2";
pub const REVIEWED_LEXICAL_ENCODING_FIELD: &str = "frm0619E:txtLineBus";
pub const CURRENT_RUST_REENCRYPTED_XML_SHA256: &str =
    "46903e58ce8b09500dc87fc63823209b8ab119990d6aee0d8073c5968a32f3a6";

/// Hash-locked native package evidence used only to decide submission safety.
pub const OFFICIAL_PACKAGE_SHA256: &str =
    "3d087545564531de1fbe8fb28f086ce6398e18608c54a0ea33353042665917eb";
pub const OFFICIAL_PACKAGE_VERSION: &str = "7.9.5.0";
pub const OFFICIAL_PACKAGE_MANIFEST_RESOURCE_ID: u32 = 129;
pub const OFFICIAL_PACKAGE_MANIFEST_FILE_OFFSET: usize = 369_216;
pub const OFFICIAL_PACKAGE_MANIFEST_SIZE: usize = 26_828;
pub const OFFICIAL_PACKAGE_MANIFEST_SHA256: &str =
    "c8811837405fd76d8924a1c04a6f283a9ed448e3792753da21aaf6ceea191249";
pub const OFFICIAL_HTA_MANIFEST_INDEX: u32 = 12;
pub const OFFICIAL_HTA_RESOURCE_ID: u32 = 141;
pub const OFFICIAL_HTA_RESOURCE_FILE_OFFSET: usize = 1_055_280;
pub const OFFICIAL_HTA_RESOURCE_DECODED_SIZE: usize = 198_185;
pub const OFFICIAL_HTA_RESOURCE_DECODED_SHA256: &str =
    "a0ef4d8958b28e63c511e7bc961e67e8ec254a6cb5f4e9f93f6d92fc9fe56f47";
pub const OFFICIAL_EBIRTOOLS_RESOURCE_ID: u32 = 553;
pub const OFFICIAL_EBIRTOOLS_RESOURCE_FILE_OFFSET: usize = 54_862_324;
pub const OFFICIAL_EBIRTOOLS_RESOURCE_DECODED_SIZE: usize = 6_451;
pub const OFFICIAL_EBIRTOOLS_RESOURCE_DECODED_SHA256: &str =
    "aaf5dbe9593ca81f808540e537353f297f9bd8638e488ea5161673e3985a91bc";
pub const OFFICIAL_ENVIRONMENT_RESOURCE_ID: u32 = 554;
pub const OFFICIAL_ENVIRONMENT_RESOURCE_FILE_OFFSET: usize = 54_868_776;
pub const OFFICIAL_ENVIRONMENT_RESOURCE_DECODED_SIZE: usize = 11_183;
pub const OFFICIAL_ENVIRONMENT_RESOURCE_DECODED_SHA256: &str =
    "01de5f90ad3c5a65af5c1ccdb61a8968d3c61e5d542eb160b6e0eb3432a3be4e";
pub const OFFICIAL_STRING_UTIL_RESOURCE_ID: u32 = 566;
pub const OFFICIAL_STRING_UTIL_RESOURCE_FILE_OFFSET: usize = 56_037_252;
pub const OFFICIAL_STRING_UTIL_RESOURCE_DECODED_SIZE: usize = 55_573;
pub const OFFICIAL_STRING_UTIL_RESOURCE_DECODED_SHA256: &str =
    "8d3f3527e044a5325b1f9019d234717d60c5bb1f72692ea302eb4f9e9cb43d6f";

/// Item 12 on the official form.  One value is authoritative; the inverse XML
/// checkbox is derived at the serialization boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum WithholdingAgentCategory {
    #[default]
    Private,
    Government,
}

/// The reviewed plain save has `txtFinalFlag=1`, while its encrypted companion
/// has `txtFinalFlag=0`.  Their lifecycle meaning is not proven.  Preserve the
/// observed value instead of pretending one of them is universally correct.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Form0619EXmlFinalFlag {
    Zero,
    #[default]
    One,
    Missing,
    Unknown(String),
}

impl Form0619EXmlFinalFlag {
    pub fn as_xml_value(&self) -> &str {
        match self {
            Self::Zero => "0",
            Self::One => "1",
            Self::Missing => "",
            Self::Unknown(value) => value,
        }
    }

    pub fn requires_review(&self) -> bool {
        matches!(self, Self::Missing | Self::Unknown(_))
    }
}

/// One of the four fixed Part III payment rows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Form0619EPaymentRow {
    pub drawee_bank_or_agency: String,
    pub number: String,
    /// Manual `MM/DD/YYYY` text.  The reviewed evidence does not prove a
    /// payment-channel-specific date rule.
    pub date: String,
    /// `None` preserves an officially blank amount cell; `Some(0.0)` is an
    /// explicitly entered zero.
    pub amount: Option<f64>,
}

impl Form0619EPaymentRow {
    pub fn is_empty(&self) -> bool {
        self.drawee_bank_or_agency.trim().is_empty()
            && self.number.trim().is_empty()
            && self.date.trim().is_empty()
            && self.amount.is_none()
    }
}

/// The official form has exactly four payment rows, not a repeatable schedule.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Form0619EPaymentDetails {
    pub cash_or_bank_debit_memo: Form0619EPaymentRow,
    pub check: Form0619EPaymentRow,
    pub tax_debit_memo: Form0619EPaymentRow,
    pub others: Form0619EPaymentRow,
    pub others_description: String,
}

/// Complete typed draft for exact identity `0619Ev2018`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Form0619EDraft {
    pub id: Option<i64>,

    // Filing period.
    pub tin: String,
    pub taxable_year: u16,
    pub month: u8,

    // Header choices.  XML yes/no pairs are derived from these values.
    pub is_amended: bool,
    #[serde(default, alias = "opt_withheld_y")]
    pub any_taxes_withheld: bool,
    #[serde(
        default,
        alias = "opt_category_g",
        deserialize_with = "deserialize_withholding_agent_category"
    )]
    pub withholding_agent_category: WithholdingAgentCategory,

    // The official PDF proves only the due-date field.  The reviewed payloads
    // contain day 10, but one payload cannot establish a universal rule.
    #[serde(default, alias = "txt_due_day")]
    pub due_day: Option<u8>,

    // Profile-prefilled semantic values.
    pub rdo_code: String,
    pub taxpayer_name: String,
    #[serde(default, alias = "txt_line_bus")]
    pub line_of_business: String,
    pub registered_address: String,
    #[serde(default)]
    pub registered_address_2: String,
    pub zip_code: String,
    pub contact_number: String,
    pub email: String,

    // Part II — Tax Remittance.  Item names match the official form.
    #[serde(alias = "txt_tax14")]
    pub item_14_amount_of_remittance: f64,
    #[serde(alias = "txt_tax15")]
    pub item_15_amount_remitted_previously: f64,
    #[serde(alias = "txt_tax16")]
    pub item_16_net_amount_of_remittance: f64,
    #[serde(alias = "txt_tax17a")]
    pub item_17a_surcharge: f64,
    #[serde(alias = "txt_tax17b")]
    pub item_17b_interest: f64,
    #[serde(alias = "txt_tax17c")]
    pub item_17c_compromise: f64,
    /// The penalty items' computed values, overrides, note and engine
    /// inputs (the shared model in [`super::penalty_items`]).
    #[serde(flatten)]
    pub penalty: PenaltyState,
    #[serde(alias = "txt_tax17d")]
    pub item_17d_total_penalties: f64,
    #[serde(alias = "txt_tax18")]
    pub item_18_total_amount_of_remittance: f64,

    // Signature/tax-agent fields present in the exact XML union.
    #[serde(default)]
    pub tax_agent_accreditation_number: String,
    #[serde(default)]
    pub tax_agent_date_of_issue: String,
    #[serde(default)]
    pub tax_agent_date_of_expiry: String,

    // Part III — four fixed official rows.
    #[serde(default)]
    pub payment_details: Form0619EPaymentDetails,

    // XML evidence.  Unmodeled/transport keys are retained and surfaced to the
    // caller; authoritative fields overwrite them during export.
    #[serde(default)]
    pub xml_final_flag: Form0619EXmlFinalFlag,
    #[serde(default)]
    pub preserved_unmodeled_xml_fields: BTreeMap<String, String>,

    /// Pre-queue builds stored their own error text here; kept so old JSON
    /// round-trips. The generic lifecycle reports `submission_error`.
    #[serde(default)]
    pub last_error: Option<String>,

    /// Status, queue authorization and retry state, flattened so stored JSON
    /// keeps the keys earlier builds wrote inline (`status`, `created_at`, ...).
    #[serde(flatten)]
    pub lifecycle: SubmissionLifecycle,
}

fn deserialize_withholding_agent_category<'de, D>(
    deserializer: D,
) -> Result<WithholdingAgentCategory, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum CategoryValue {
        Current(WithholdingAgentCategory),
        LegacyGovernmentFlag(bool),
    }

    Ok(match CategoryValue::deserialize(deserializer)? {
        CategoryValue::Current(value) => value,
        CategoryValue::LegacyGovernmentFlag(true) => WithholdingAgentCategory::Government,
        CategoryValue::LegacyGovernmentFlag(false) => WithholdingAgentCategory::Private,
    })
}

impl Form0619EDraft {
    pub fn new_from_profile(profile: &TaxpayerProfile, year: u16, month: u8) -> Self {
        let mut draft = Self {
            penalty: crate::forms::penalty_items::PenaltyState::for_profile(profile),
            id: None,
            tin: profile.tin.full(),
            taxable_year: year,
            month,
            is_amended: false,
            any_taxes_withheld: false,
            withholding_agent_category: if profile.is_government_withholding_entity {
                WithholdingAgentCategory::Government
            } else {
                WithholdingAgentCategory::Private
            },
            // The official page's default (`txtDueDay` value 10).
            due_day: Some(10),
            rdo_code: profile.rdo_code.clone(),
            taxpayer_name: profile.full_name.clone(),
            line_of_business: profile.line_of_business.clone(),
            registered_address: profile.registered_address.clone(),
            registered_address_2: String::new(),
            zip_code: profile.zip_code.clone(),
            contact_number: profile.phone.clone(),
            email: profile.email.clone(),
            item_14_amount_of_remittance: 0.0,
            item_15_amount_remitted_previously: 0.0,
            item_16_net_amount_of_remittance: 0.0,
            item_17a_surcharge: 0.0,
            item_17b_interest: 0.0,
            item_17c_compromise: 0.0,
            item_17d_total_penalties: 0.0,
            item_18_total_amount_of_remittance: 0.0,
            tax_agent_accreditation_number: String::new(),
            tax_agent_date_of_issue: String::new(),
            tax_agent_date_of_expiry: String::new(),
            payment_details: Form0619EPaymentDetails::default(),
            // Value one is the reviewed editable/plain-save value.  We never
            // reuse it as evidence for an encrypted submission payload.
            xml_final_flag: Form0619EXmlFinalFlag::One,
            preserved_unmodeled_xml_fields: BTreeMap::new(),
            last_error: None,
            lifecycle: SubmissionLifecycle::default(),
        };
        draft.recompute();
        draft
    }

    pub const fn atc_code(&self) -> &'static str {
        ATC_CODE
    }

    pub const fn tax_type_code(&self) -> &'static str {
        TAX_TYPE_CODE
    }

    /// Month/year of the due-date field.  This rollover is arithmetic evidence,
    /// not a claim about the legally correct due day.
    pub fn due_month_and_year(&self) -> (u8, u16) {
        if self.month == 12 {
            (1, self.taxable_year.saturating_add(1))
        } else {
            (self.month.saturating_add(1), self.taxable_year)
        }
    }

    /// The official compute chain (see `form_0619e_official`).
    pub fn recompute(&mut self) {
        self.official_recompute();
    }

    pub fn is_editable(&self) -> bool {
        self.lifecycle.is_editable()
    }

    pub fn xml_evidence_warnings(&self) -> Vec<String> {
        let mut warnings = vec![
            "The reviewed plain 0619-E save uses txtFinalFlag=1 while its encrypted companion uses txtFinalFlag=0; the observed value is preserved and no submission meaning is inferred."
                .to_string(),
            "Rust decrypts and semantically replays the reviewed encrypted companion, but the current Rust compression writer does not reproduce the locked ciphertext byte-for-byte; exact outbound generation is not certified."
                .to_string(),
            "Queue submission remains disabled: eBIRForms 7.9.5.0 fetches endpoint, mode, port, username, and password from the BIR dispatcher at runtime, then invokes Encrypt.exe and cFTPSend.exe, which are absent from the reviewed package manifest."
                .to_string(),
            "The native helper exit code reports upload completion only and the HTA says submission remains subject to BIR validation; no reviewed 0619-E confirmation response or durable queue-claim persistence contract exists."
                .to_string(),
        ];
        if self.xml_final_flag.requires_review() {
            warnings.push(format!(
                "txtFinalFlag value {:?} is outside the two reviewed source values",
                self.xml_final_flag
            ));
        }
        if !self.preserved_unmodeled_xml_fields.is_empty() {
            warnings.push(format!(
                "Preserved unmodeled XML keys require review: {}",
                self.preserved_unmodeled_xml_fields
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        warnings
    }
}

impl FormValidator for Form0619EDraft {
    /// The official `validateForm` port (see `form_0619e_official`).
    fn validate(&self) -> Vec<(String, String)> {
        let mut errors = self.official_errors();
        if self.xml_final_flag.requires_review() {
            errors.push((
                "xml_final_flag".to_string(),
                "The imported txtFinalFlag value is unreviewed and cannot be exported safely"
                    .to_string(),
            ));
        }
        errors
    }
}

impl TypedBirForm for Form0619EDraft {
    fn form_code(&self) -> &'static str {
        FORM_CODE
    }

    fn form_type_id(&self) -> &'static str {
        FORM_TYPE_ID
    }

    fn filing_period(&self) -> FilingPeriod {
        FilingPeriod::Monthly(self.month)
    }

    fn recompute(&mut self) {
        Form0619EDraft::recompute(self);
    }

    fn to_bir_field_map(&self) -> BTreeMap<String, String> {
        Form0619EDraft::to_bir_field_map(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::naming::Tin;

    fn test_profile() -> TaxpayerProfile {
        serde_json::from_value(serde_json::json!({
            "id": null,
            "full_name": "JUAN DELA CRUZ",
            "tin": {
                "segment1": "123",
                "segment2": "456",
                "segment3": "788",
                "branch": "00000"
            },
            "rdo_code": "018",
            "line_of_business": "SOFTWARE DEVELOPMENT",
            "registered_address": "OLONGAPO",
            "zip_code": "2200",
            "phone": "09123456789",
            "email": "taxpayer@example.com",
            "default_form_type": "0619Ev2018",
            "taxpayer_type": "Individual"
        }))
        .expect("test profile must deserialize")
    }

    fn valid_draft() -> Form0619EDraft {
        let mut draft = Form0619EDraft::new_from_profile(&test_profile(), 2026, 4);
        draft.due_day = Some(10);
        draft.any_taxes_withheld = true;
        draft.item_14_amount_of_remittance = 1_000.0;
        draft.item_17a_surcharge = 100.0;
        draft.item_17b_interest = 30.0;
        draft.item_17c_compromise = 100.0;
        crate::forms::queueable::QueueableForm::keep_entered_penalties(&mut draft);
        draft.recompute();
        draft
    }

    #[test]
    fn exact_identity_and_fixed_codes_are_not_mutable_draft_fields() {
        let draft = valid_draft();
        assert_eq!(draft.form_type_id(), "0619Ev2018");
        assert_eq!(draft.atc_code(), "WME10");
        assert_eq!(draft.tax_type_code(), "WE");
    }

    #[test]
    fn official_formulas_are_deterministic_and_do_not_rewrite_timestamps() {
        let mut draft = valid_draft();
        draft.lifecycle.updated_at = "fixed".to_string();
        draft.recompute();
        assert_eq!(draft.item_16_net_amount_of_remittance, 1_000.0);
        assert_eq!(draft.item_17d_total_penalties, 230.0);
        assert_eq!(draft.item_18_total_amount_of_remittance, 1_230.0);
        assert_eq!(draft.lifecycle.updated_at, "fixed");
    }

    #[test]
    fn item_16_is_not_clamped_when_item_15_exceeds_item_14() {
        let mut draft = valid_draft();
        draft.is_amended = true;
        draft.item_15_amount_remitted_previously = 1_500.0;
        draft.recompute();
        // computeNetAmtRem formats the negative difference as is.
        assert_eq!(draft.item_16_net_amount_of_remittance, -500.0);
    }

    #[test]
    fn legacy_manual_penalties_load_as_overrides_and_keep_their_totals() {
        // Before the computed-default model, Items 17A-17C were typed by
        // hand and never replaced by a clock-based engine. A draft saved then
        // (no `penalty_model_version`) keeps every non-zero hand-typed value
        // as an override, so nothing it carried changes value; a zero item
        // follows the engine like any other.
        let mut current = valid_draft();
        current.item_17a_surcharge = 7.0;
        current.item_17b_interest = 0.0;
        current.item_17c_compromise = 9.0;
        let mut json = serde_json::to_value(&current).unwrap();
        let object = json.as_object_mut().unwrap();
        for key in [
            "penalty_model_version",
            "penalty_due_date",
            "penalty_taxpayer_class",
            "computed_penalties",
            "penalty_overrides",
        ] {
            object.remove(key);
        }
        let mut draft: Form0619EDraft = serde_json::from_value(json).unwrap();
        assert_eq!(draft.penalty.penalty_model_version, 0);

        let late = chrono::NaiveDate::from_ymd_opt(2026, 10, 13).unwrap();
        crate::forms::penalty_items::with_clock(late, || draft.recompute());

        let overrides = &draft.penalty.penalty_overrides;
        assert_eq!(overrides.surcharge, Some(7.0));
        assert_eq!(overrides.interest, None);
        assert_eq!(overrides.compromise, Some(9.0));
        assert_eq!(draft.item_17a_surcharge, 7.0);
        assert_eq!(draft.item_17c_compromise, 9.0);
        let computed = draft
            .penalty
            .computed_penalties
            .clone()
            .expect("the late period's computed values are still shown");
        assert_eq!(draft.item_17b_interest, computed.interest);
        assert_eq!(
            draft.item_17d_total_penalties,
            ((7.0 + computed.interest + 9.0) * 100.0_f64).round() / 100.0
        );

        // Later recomputes keep the overrides.
        let next_day = chrono::NaiveDate::from_ymd_opt(2026, 10, 14).unwrap();
        crate::forms::penalty_items::with_clock(next_day, || draft.recompute());
        assert_eq!(draft.item_17a_surcharge, 7.0);
        assert_eq!(draft.item_17c_compromise, 9.0);
    }

    #[test]
    fn december_due_period_rolls_into_the_next_year() {
        let draft = Form0619EDraft::new_from_profile(&test_profile(), 2026, 12);
        assert_eq!(draft.due_month_and_year(), (1, 2027));
    }

    #[test]
    fn due_day_defaults_to_the_official_page_value_and_missing_fails() {
        let mut draft = Form0619EDraft::new_from_profile(&test_profile(), 2026, 4);
        assert_eq!(draft.due_day, Some(10));
        draft.due_day = None;
        assert!(draft.validate().iter().any(|(field, message)| {
            field == "due_day" && message == "Please enter a valid Date on Item 2"
        }));
    }

    #[test]
    fn payment_details_have_exactly_four_named_rows() {
        let mut draft = valid_draft();
        draft.payment_details.cash_or_bank_debit_memo.number = "BDM-1".to_string();
        draft.payment_details.check.number = "CHECK-2".to_string();
        draft.payment_details.tax_debit_memo.number = "TDM-3".to_string();
        draft.payment_details.others.number = "OTHER-4".to_string();
        draft.payment_details.others_description = "MANUAL PAYMENT".to_string();
        assert_eq!(
            draft.payment_details.cash_or_bank_debit_memo.number,
            "BDM-1"
        );
        assert_eq!(draft.payment_details.check.number, "CHECK-2");
        assert_eq!(draft.payment_details.tax_debit_memo.number, "TDM-3");
        assert_eq!(draft.payment_details.others.number, "OTHER-4");
    }

    #[test]
    fn json_roundtrip_preserves_typed_payment_rows_and_evidence() {
        let mut draft = valid_draft();
        draft.payment_details.others = Form0619EPaymentRow {
            drawee_bank_or_agency: "AAB".to_string(),
            number: "REF".to_string(),
            date: "05/10/2026".to_string(),
            amount: Some(1_230.0),
        };
        draft.payment_details.others_description = "OTHER CHANNEL".to_string();
        draft
            .preserved_unmodeled_xml_fields
            .insert("source:futureKey".to_string(), "PRESERVE ME".to_string());
        let json = serde_json::to_string(&draft).expect("draft should serialize");
        let reopened: Form0619EDraft =
            serde_json::from_str(&json).expect("draft should deserialize");
        assert_eq!(reopened, draft);
    }

    #[test]
    fn legacy_scaffold_json_aliases_migrate_to_semantic_fields() {
        let draft = valid_draft();
        let mut value = serde_json::to_value(&draft).expect("draft should serialize");
        let object = value.as_object_mut().expect("draft JSON is an object");
        object.insert("opt_category_g".to_string(), serde_json::json!(true));
        object.remove("withholding_agent_category");
        object.insert("txt_due_day".to_string(), serde_json::json!(10));
        object.remove("due_day");
        object.insert(
            "txt_line_bus".to_string(),
            serde_json::json!("LEGACY BUSINESS"),
        );
        object.remove("line_of_business");
        for (legacy, current) in [
            ("txt_tax14", "item_14_amount_of_remittance"),
            ("txt_tax15", "item_15_amount_remitted_previously"),
            ("txt_tax16", "item_16_net_amount_of_remittance"),
            ("txt_tax17a", "item_17a_surcharge"),
            ("txt_tax17b", "item_17b_interest"),
            ("txt_tax17c", "item_17c_compromise"),
            ("txt_tax17d", "item_17d_total_penalties"),
            ("txt_tax18", "item_18_total_amount_of_remittance"),
        ] {
            let field_value = object.remove(current).expect("current field exists");
            object.insert(legacy.to_string(), field_value);
        }

        let migrated: Form0619EDraft =
            serde_json::from_value(value).expect("legacy aliases should deserialize");
        assert_eq!(migrated.due_day, Some(10));
        assert_eq!(migrated.line_of_business, "LEGACY BUSINESS");
        assert_eq!(
            migrated.withholding_agent_category,
            WithholdingAgentCategory::Government
        );
        assert_eq!(migrated.item_18_total_amount_of_remittance, 1_230.0);
    }

    #[test]
    fn test_profile_fixture_uses_supported_tin_shape() {
        let profile = test_profile();
        assert_eq!(
            profile.tin.full(),
            Tin {
                segment1: "123".into(),
                segment2: "456".into(),
                segment3: "788".into(),
                branch: "00000".into(),
            }
            .full()
        );
    }
}
