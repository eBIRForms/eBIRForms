use bir_core::calendar_rules::{DeadlineKind, DeadlineOverride, DeadlinePeriod, DeadlineResolver};
use bir_core::integration::recurring_obligation_forms_for_profile_and_year;
use bir_core::integration::{
    deadline_applies_to_profile, profile_deadline_overrides_for_year,
    resolve_profile_obligations_for_year_with_global_overrides,
};
use bir_core::naming::Tin;
use bir_core::profile::{
    ComplianceSourceMode, EoptTier, ExciseTaxCategory, IncomeTaxElection, ManualObligationOverride,
    ManualObligationOverrideAction, ProfileDeadlineOverride, RegisteredTaxType,
    RegistrationActivityStatus, TaxClassification, TaxElectionHistory, TaxProfileVersion,
    TaxProfileVersionStatus, TaxpayerProfile, TaxpayerType,
};
use bir_core::validation::validate_profile;
use chrono::NaiveDate;
use std::collections::BTreeSet;

const TAXABLE_YEAR: u16 = 2026;
// One annual ITR per year: the primary is selected by classification, EOPT
// tier, and deduction election. Self-employed defaults to 1701; an 8%/OSD
// election maps to 1701A; a Micro/Small tier (2024+) without such an election
// maps to the simplified 1701MS.
const SELF_EMPLOYED_NON_VAT: &[&str] = &["1701", "1701Q", "2551Q"];
const SELF_EMPLOYED_NON_VAT_MICRO_SMALL: &[&str] = &["1701MS", "1701Q", "2551Q"];
const SELF_EMPLOYED_NON_VAT_8_PERCENT: &[&str] = &["1701A", "1701Q"];
const SELF_EMPLOYED_NON_VAT_MICRO_SMALL_8_PERCENT: &[&str] = &["1701A", "1701Q"];
const SELF_EMPLOYED_VAT: &[&str] = &["1701", "1701Q", "2550Q"];
const SELF_EMPLOYED_VAT_MICRO_SMALL: &[&str] = &["1701MS", "1701Q", "2550Q"];
const SELF_EMPLOYED_VAT_8_PERCENT: &[&str] = &["1701A", "1701Q", "2550Q"];
const SELF_EMPLOYED_VAT_MICRO_SMALL_8_PERCENT: &[&str] = &["1701A", "1701Q", "2550Q"];
const MIXED_NON_VAT: &[&str] = &["1701", "1701Q", "2551Q"];
const MIXED_NON_VAT_8_PERCENT: &[&str] = &["1701", "1701Q"];
const MIXED_VAT: &[&str] = &["1701", "1701Q", "2550Q"];
const COMPENSATION_WITHHOLDING: &[&str] = &["0620", "1601C", "1604C", "2316"];
const EXPANDED_WITHHOLDING: &[&str] = &["0619E", "1601EQ", "1604E"];
const FINAL_WITHHOLDING: &[&str] = &[
    "0619F", "1600WP", "1601F", "1601FQ", "1602", "1603", "1604F",
];

fn base_profile(
    taxpayer_type: TaxpayerType,
    classification: Option<TaxClassification>,
) -> TaxpayerProfile {
    TaxpayerProfile {
        id: Some(1),
        full_name: "Matrix Test".into(),
        tin: Tin {
            segment1: "000".into(),
            segment2: "234".into(),
            segment3: "564".into(),
            branch: "000".into(),
        },
        rdo_code: "039".into(),
        line_of_business: "Matrix".into(),
        registered_address: "QC".into(),
        zip_code: "1100".into(),
        phone: "09170000100".into(),
        email: "matrix@example.com".into(),
        default_form_type: "2551Q".into(),
        taxpayer_type,
        is_vat_registered: false,
        business_start_date: Some(NaiveDate::from_ymd_opt(2020, 1, 1).unwrap()),
        tax_classification: classification,
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
        is_archived: false,
        profile_pin_hash: None,
        email_tracking_enabled: false,
        test_notification_enabled: false,
        profile_versions: vec![],
        birth_date: None,
        compliance_source_mode: Default::default(),
        per_year_forms: Default::default(),
    }
}

fn forms_for(profile: &TaxpayerProfile) -> BTreeSet<String> {
    forms_for_year(profile, TAXABLE_YEAR)
}

fn forms_for_year(profile: &TaxpayerProfile, year: u16) -> BTreeSet<String> {
    let mut p = profile.clone();
    p.ensure_profile_version_ledger();
    configure_forms_set_for_year(&mut p, year);
    recurring_obligation_forms_for_profile_and_year(&p, year)
        .into_iter()
        .collect()
}

fn configure_forms_set_for_year(profile: &mut TaxpayerProfile, year: u16) {
    let suggestions = bir_core::integration::form_suggestions_for_profile_year(profile, year);
    let reconciliation = bir_core::forms::reconcile_forms_set_for_year(
        year,
        profile.per_year_forms.get(&year),
        &suggestions,
    );
    profile
        .per_year_forms
        .insert(year, reconciliation.forms_set);
}

fn expected(codes: &[&str]) -> BTreeSet<String> {
    codes.iter().map(|code| code.to_string()).collect()
}

fn assert_forms(label: &str, profile: &TaxpayerProfile, codes: &[&str]) {
    assert_eq!(forms_for(profile), expected(codes), "{label}");
}

fn assert_forms_union(label: &str, profile: &TaxpayerProfile, groups: &[&[&str]]) {
    let mut codes = BTreeSet::new();
    for group in groups {
        codes.extend(expected(group));
    }

    assert_eq!(forms_for(profile), codes, "{label}");
}

fn add_eight_percent_election(profile: &mut TaxpayerProfile, year: u16) {
    profile.tax_elections.push(TaxElectionHistory {
        taxable_year: year,
        election: IncomeTaxElection::EightPercent,
        elected_at: chrono::NaiveDateTime::default(),
        source_form: "matrix".into(),
    });
}

fn self_employed_profile(
    vat: bool,
    tier: Option<EoptTier>,
    eight_percent: bool,
) -> TaxpayerProfile {
    let mut profile = base_profile(
        TaxpayerType::Individual,
        Some(TaxClassification::SelfEmployed),
    );
    profile.is_vat_registered = vat;
    profile.eopt_tier = tier;

    if eight_percent {
        add_eight_percent_election(&mut profile, TAXABLE_YEAR);
    }

    profile
}

fn confirmed_version(
    profile: &TaxpayerProfile,
    id: &str,
    label: &str,
    from: Option<(i32, u32, u32)>,
    until: Option<(i32, u32, u32)>,
    tax_types: Vec<RegisteredTaxType>,
    vat: bool,
) -> TaxProfileVersion {
    let mut version = TaxProfileVersion::from_profile_backfill(profile);
    version.id = id.to_string();
    version.label = label.to_string();
    version.status = TaxProfileVersionStatus::Confirmed;
    version.effective_from = from.map(|(y, m, d)| NaiveDate::from_ymd_opt(y, m, d).unwrap());
    version.effective_until = until.map(|(y, m, d)| NaiveDate::from_ymd_opt(y, m, d).unwrap());
    version.needs_effective_date_review = version.effective_from.is_none();
    version.registered_tax_types = tax_types;
    version.is_vat_registered = vat;
    version
}

#[test]
fn dashboard_profile_matrix_base_forms_for_2026() {
    let pure_comp = base_profile(
        TaxpayerType::Individual,
        Some(TaxClassification::PurelyCompensation),
    );
    assert_forms("pure compensation", &pure_comp, &["1700"]);

    let mut pure_comp_single = pure_comp.clone();
    pure_comp_single.has_single_employer = true;
    assert_forms("pure compensation single employer", &pure_comp_single, &[]);

    assert_forms(
        "self-employed non-vat no tier",
        &self_employed_profile(false, None, false),
        SELF_EMPLOYED_NON_VAT,
    );
    assert_forms(
        "self-employed non-vat micro",
        &self_employed_profile(false, Some(EoptTier::Micro), false),
        SELF_EMPLOYED_NON_VAT_MICRO_SMALL,
    );
    assert_forms(
        "self-employed non-vat small",
        &self_employed_profile(false, Some(EoptTier::Small), false),
        SELF_EMPLOYED_NON_VAT_MICRO_SMALL,
    );
    assert_forms(
        "self-employed non-vat medium",
        &self_employed_profile(false, Some(EoptTier::Medium), false),
        SELF_EMPLOYED_NON_VAT,
    );
    assert_forms(
        "self-employed non-vat large",
        &self_employed_profile(false, Some(EoptTier::Large), false),
        SELF_EMPLOYED_NON_VAT,
    );
    assert_forms(
        "self-employed non-vat 8 percent",
        &self_employed_profile(false, None, true),
        SELF_EMPLOYED_NON_VAT_8_PERCENT,
    );
    assert_forms(
        "self-employed non-vat micro 8 percent",
        &self_employed_profile(false, Some(EoptTier::Micro), true),
        SELF_EMPLOYED_NON_VAT_MICRO_SMALL_8_PERCENT,
    );
    assert_forms(
        "self-employed non-vat small 8 percent",
        &self_employed_profile(false, Some(EoptTier::Small), true),
        SELF_EMPLOYED_NON_VAT_MICRO_SMALL_8_PERCENT,
    );
    assert_forms(
        "self-employed non-vat medium 8 percent",
        &self_employed_profile(false, Some(EoptTier::Medium), true),
        SELF_EMPLOYED_NON_VAT_8_PERCENT,
    );
    assert_forms(
        "self-employed non-vat large 8 percent",
        &self_employed_profile(false, Some(EoptTier::Large), true),
        SELF_EMPLOYED_NON_VAT_8_PERCENT,
    );

    assert_forms(
        "self-employed vat no tier",
        &self_employed_profile(true, None, false),
        SELF_EMPLOYED_VAT,
    );
    assert_forms(
        "self-employed vat micro",
        &self_employed_profile(true, Some(EoptTier::Micro), false),
        SELF_EMPLOYED_VAT_MICRO_SMALL,
    );
    assert_forms(
        "self-employed vat small",
        &self_employed_profile(true, Some(EoptTier::Small), false),
        SELF_EMPLOYED_VAT_MICRO_SMALL,
    );
    assert_forms(
        "self-employed vat medium",
        &self_employed_profile(true, Some(EoptTier::Medium), false),
        SELF_EMPLOYED_VAT,
    );
    assert_forms(
        "self-employed vat large",
        &self_employed_profile(true, Some(EoptTier::Large), false),
        SELF_EMPLOYED_VAT,
    );
    assert_forms(
        "self-employed vat 8 percent",
        &self_employed_profile(true, None, true),
        SELF_EMPLOYED_VAT_8_PERCENT,
    );
    assert_forms(
        "self-employed vat micro 8 percent",
        &self_employed_profile(true, Some(EoptTier::Micro), true),
        SELF_EMPLOYED_VAT_MICRO_SMALL_8_PERCENT,
    );
    assert_forms(
        "self-employed vat small 8 percent",
        &self_employed_profile(true, Some(EoptTier::Small), true),
        SELF_EMPLOYED_VAT_MICRO_SMALL_8_PERCENT,
    );
    assert_forms(
        "self-employed vat medium 8 percent",
        &self_employed_profile(true, Some(EoptTier::Medium), true),
        SELF_EMPLOYED_VAT_8_PERCENT,
    );
    assert_forms(
        "self-employed vat large 8 percent",
        &self_employed_profile(true, Some(EoptTier::Large), true),
        SELF_EMPLOYED_VAT_8_PERCENT,
    );

    let mut mixed_non_vat = base_profile(
        TaxpayerType::Individual,
        Some(TaxClassification::MixedIncome),
    );
    assert_forms("mixed income non-vat", &mixed_non_vat, MIXED_NON_VAT);
    mixed_non_vat.eopt_tier = Some(EoptTier::Micro);
    assert_forms(
        "mixed income non-vat micro tier",
        &mixed_non_vat,
        MIXED_NON_VAT,
    );
    add_eight_percent_election(&mut mixed_non_vat, TAXABLE_YEAR);
    assert_forms(
        "mixed income non-vat 8 percent",
        &mixed_non_vat,
        MIXED_NON_VAT_8_PERCENT,
    );
    mixed_non_vat.is_vat_registered = true;
    assert_forms("mixed income vat", &mixed_non_vat, MIXED_VAT);

    let corp_non_vat = base_profile(TaxpayerType::Corporation, None);
    assert_forms(
        "corporation non-vat",
        &corp_non_vat,
        &["1702Q", "1702RT", "2551Q"],
    );
    let mut corp_vat = corp_non_vat.clone();
    corp_vat.is_vat_registered = true;
    assert_forms("corporation vat", &corp_vat, &["1702Q", "1702RT", "2550Q"]);

    let partnership_non_vat = base_profile(TaxpayerType::Partnership, None);
    assert_forms(
        "partnership non-vat",
        &partnership_non_vat,
        &["1702Q", "1702RT", "2551Q"],
    );
    let mut partnership_vat = partnership_non_vat.clone();
    partnership_vat.is_vat_registered = true;
    assert_forms(
        "partnership vat",
        &partnership_vat,
        &["1702Q", "1702RT", "2550Q"],
    );

    assert_forms(
        "cooperative exempt",
        &base_profile(
            TaxpayerType::Cooperative,
            Some(TaxClassification::CooperativeExempt),
        ),
        &["1702EX", "1702Q"],
    );
    assert_forms(
        "cooperative taxable",
        &base_profile(
            TaxpayerType::Cooperative,
            Some(TaxClassification::CooperativeTaxable),
        ),
        &["1702Q", "1702RT"],
    );
    assert_forms(
        "cooperative mixed",
        &base_profile(
            TaxpayerType::Cooperative,
            Some(TaxClassification::CooperativeMixed),
        ),
        &["1702MX", "1702Q"],
    );
    assert_forms(
        "cooperative default treatment",
        &base_profile(TaxpayerType::Cooperative, None),
        &["1702Q", "1702RT"],
    );

    assert_forms(
        "estate",
        &base_profile(TaxpayerType::Estate, None),
        &["1701", "1701Q"],
    );
    assert_forms(
        "trust",
        &base_profile(TaxpayerType::Trust, None),
        &["1701", "1701Q"],
    );
}

#[test]
fn dashboard_profile_matrix_withholding_modifiers() {
    let base = self_employed_profile(false, None, false);

    let mut compensation = base.clone();
    compensation.withholds_compensation = true;
    assert_forms_union(
        "withholding compensation",
        &compensation,
        &[SELF_EMPLOYED_NON_VAT, COMPENSATION_WITHHOLDING],
    );

    let mut legacy_compensation = self_employed_profile(false, None, false);
    legacy_compensation.has_employees = true;
    assert_forms_union(
        "legacy has employees maps to compensation withholding",
        &legacy_compensation,
        &[SELF_EMPLOYED_NON_VAT, COMPENSATION_WITHHOLDING],
    );

    let mut expanded = base.clone();
    expanded.withholds_expanded = true;
    assert_forms_union(
        "withholding expanded",
        &expanded,
        &[SELF_EMPLOYED_NON_VAT, EXPANDED_WITHHOLDING],
    );

    let mut legacy_expanded = self_employed_profile(false, None, false);
    legacy_expanded.is_expanded_withholding_agent = true;
    assert_forms_union(
        "legacy expanded withholding agent maps to expanded withholding",
        &legacy_expanded,
        &[SELF_EMPLOYED_NON_VAT, EXPANDED_WITHHOLDING],
    );

    let mut top_withholding_agent = self_employed_profile(false, None, false);
    top_withholding_agent.is_top_withholding_agent = true;
    assert_forms_union(
        "top withholding agent maps to expanded withholding",
        &top_withholding_agent,
        &[SELF_EMPLOYED_NON_VAT, EXPANDED_WITHHOLDING],
    );

    let mut government_withholding_entity = self_employed_profile(false, None, false);
    government_withholding_entity.is_government_withholding_entity = true;
    assert_forms_union(
        "government withholding entity maps to expanded withholding",
        &government_withholding_entity,
        &[SELF_EMPLOYED_NON_VAT, EXPANDED_WITHHOLDING],
    );

    let mut final_wh = base.clone();
    final_wh.withholds_final = true;
    assert_forms_union(
        "withholding final",
        &final_wh,
        &[SELF_EMPLOYED_NON_VAT, FINAL_WITHHOLDING],
    );

    let mut all = base;
    all.withholds_compensation = true;
    all.withholds_expanded = true;
    all.withholds_final = true;
    assert_forms_union(
        "all withholding",
        &all,
        &[
            SELF_EMPLOYED_NON_VAT,
            COMPENSATION_WITHHOLDING,
            EXPANDED_WITHHOLDING,
            FINAL_WITHHOLDING,
        ],
    );

    let mut single_employer = base_profile(
        TaxpayerType::Individual,
        Some(TaxClassification::PurelyCompensation),
    );
    single_employer.has_single_employer = true;
    single_employer.withholds_compensation = true;
    assert_forms(
        "pure compensation single employer with compensation withholding",
        &single_employer,
        COMPENSATION_WITHHOLDING,
    );
}

#[test]
fn dashboard_profile_matrix_registration_status_modifiers() {
    let active = self_employed_profile(false, None, false);
    assert_forms(
        "active registration keeps recurring obligations",
        &active,
        SELF_EMPLOYED_NON_VAT,
    );

    let mut dormant_operational = active.clone();
    dormant_operational.registration_activity_status =
        RegistrationActivityStatus::DormantOperational;
    assert_forms(
        "dormant operational keeps NIL filing obligations",
        &dormant_operational,
        SELF_EMPLOYED_NON_VAT,
    );

    let mut legacy_dormant = active.clone();
    legacy_dormant.is_dormant = true;
    assert_forms(
        "legacy dormant flag keeps NIL filing obligations",
        &legacy_dormant,
        SELF_EMPLOYED_NON_VAT,
    );

    let mut temporarily_inactive = active.clone();
    temporarily_inactive.registration_activity_status =
        RegistrationActivityStatus::TemporarilyInactive;
    assert_forms(
        "temporarily inactive keeps NIL filing obligations",
        &temporarily_inactive,
        SELF_EMPLOYED_NON_VAT,
    );

    let mut officially_closed = active;
    officially_closed.registration_activity_status = RegistrationActivityStatus::OfficiallyClosed;
    assert_forms(
        "officially closed has no recurring obligations",
        &officially_closed,
        &[],
    );

    let mut closed_with_withholding = self_employed_profile(false, None, false);
    closed_with_withholding.withholds_compensation = true;
    closed_with_withholding.withholds_expanded = true;
    closed_with_withholding.withholds_final = true;
    closed_with_withholding.registration_activity_status =
        RegistrationActivityStatus::OfficiallyClosed;
    assert_forms(
        "official closure suppresses income and withholding obligations",
        &closed_with_withholding,
        &[],
    );
}

#[test]
fn dashboard_profile_matrix_excise_modifiers() {
    let cases = [
        (ExciseTaxCategory::Alcohol, "2200A"),
        (ExciseTaxCategory::AutomobilesAndNonEssential, "2200AN"),
        (ExciseTaxCategory::Mineral, "2200M"),
        (ExciseTaxCategory::Petroleum, "2200P"),
        (ExciseTaxCategory::Tobacco, "2200T"),
    ];

    for (category, form_code) in &cases {
        let mut profile = self_employed_profile(false, None, false);
        profile.excise_tax_categories = vec![category.clone()];
        assert_forms(
            &format!("excise category {:?}", category),
            &profile,
            &["1701", "1701Q", *form_code, "2551Q"],
        );
    }

    let mut all_excise = self_employed_profile(false, None, false);
    all_excise.excise_tax_categories = cases.iter().map(|(category, _)| category.clone()).collect();
    assert_forms(
        "all excise categories",
        &all_excise,
        &[
            "1701", "1701Q", "2200A", "2200AN", "2200M", "2200P", "2200T", "2551Q",
        ],
    );
}

#[test]
fn versioned_cor_uses_the_profile_active_for_the_selected_year() {
    let mut profile = self_employed_profile(false, None, false);

    let non_vat_2025 = confirmed_version(
        &profile,
        "cor-2025",
        "2025 non-VAT COR",
        Some((2025, 1, 1)),
        Some((2025, 12, 31)),
        vec![
            RegisteredTaxType::IncomeTax,
            RegisteredTaxType::PercentageTax,
            RegisteredTaxType::RegistrationFee,
        ],
        false,
    );
    let vat_2026 = confirmed_version(
        &profile,
        "cor-2026",
        "2026 VAT COR",
        Some((2026, 1, 1)),
        None,
        vec![
            RegisteredTaxType::IncomeTax,
            RegisteredTaxType::ValueAddedTax,
            RegisteredTaxType::RegistrationFee,
        ],
        true,
    );
    profile.profile_versions = vec![non_vat_2025, vat_2026];
    profile.compliance_source_mode = ComplianceSourceMode::CorVersioned;

    let forms_2025 = forms_for_year(&profile, 2025);
    let forms_2026 = forms_for(&profile);

    assert!(forms_2025.contains("2551Q"));
    assert!(!forms_2025.contains("2550Q"));
    assert!(forms_2026.contains("2550Q"));
    assert!(!forms_2026.contains("2551Q"));
}

#[test]
fn cor_versioned_mode_requires_confirmed_versions() {
    let mut profile = self_employed_profile(false, None, false);
    profile.compliance_source_mode = ComplianceSourceMode::CorVersioned;

    assert!(recurring_obligation_forms_for_profile_and_year(&profile, TAXABLE_YEAR).is_empty());
    assert!(
        validate_profile(&profile)
            .iter()
            .any(|error| error.field == "profile_versions")
    );
}

#[test]
fn confirming_new_cor_version_auto_closes_previous_version() {
    let mut profile = self_employed_profile(false, None, false);
    let current = confirmed_version(
        &profile,
        "current",
        "Current COR",
        Some((2025, 1, 1)),
        None,
        vec![
            RegisteredTaxType::IncomeTax,
            RegisteredTaxType::PercentageTax,
            RegisteredTaxType::RegistrationFee,
        ],
        false,
    );
    let mut draft = confirmed_version(
        &profile,
        "draft-vat",
        "Draft VAT COR",
        Some((2026, 1, 1)),
        None,
        vec![
            RegisteredTaxType::IncomeTax,
            RegisteredTaxType::ValueAddedTax,
            RegisteredTaxType::RegistrationFee,
        ],
        true,
    );
    draft.status = TaxProfileVersionStatus::Draft;
    profile.profile_versions = vec![current, draft];

    assert!(
        profile.set_profile_version_confirmed(
            "draft-vat",
            NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()
        )
    );

    let previous = profile
        .profile_versions
        .iter()
        .find(|version| version.id == "current")
        .unwrap();
    let confirmed = profile
        .profile_versions
        .iter()
        .find(|version| version.id == "draft-vat")
        .unwrap();

    assert_eq!(
        previous.effective_until,
        Some(NaiveDate::from_ymd_opt(2025, 12, 31).unwrap())
    );
    assert_eq!(confirmed.status, TaxProfileVersionStatus::Confirmed);
    assert_eq!(
        profile.compliance_source_mode,
        ComplianceSourceMode::CorVersioned
    );
}

#[test]
fn missing_forms_set_blocks_deadlines_even_when_profile_version_is_effective() {
    let mut profile = self_employed_profile(false, None, false);
    let version = confirmed_version(
        &profile,
        "cor-june-2026",
        "June 2026 registration",
        Some((2026, 6, 1)),
        None,
        vec![
            RegisteredTaxType::IncomeTax,
            RegisteredTaxType::PercentageTax,
            RegisteredTaxType::RegistrationFee,
        ],
        false,
    );
    profile.profile_versions = vec![version];
    profile.compliance_source_mode = ComplianceSourceMode::CorVersioned;

    let deadlines = DeadlineResolver::resolve_taxable_year(2026);
    let q1_1701q = deadlines
        .iter()
        .find(|deadline| {
            deadline.form_code == "1701Q"
                && matches!(
                    deadline.period,
                    DeadlinePeriod::Quarterly {
                        taxable_year: 2026,
                        quarter: 1
                    }
                )
        })
        .unwrap();
    let q2_1701q = deadlines
        .iter()
        .find(|deadline| {
            deadline.form_code == "1701Q"
                && matches!(
                    deadline.period,
                    DeadlinePeriod::Quarterly {
                        taxable_year: 2026,
                        quarter: 2
                    }
                )
        })
        .unwrap();

    assert!(!deadline_applies_to_profile(&profile, q1_1701q));
    assert!(!deadline_applies_to_profile(&profile, q2_1701q));
}

#[test]
fn stored_forms_set_controls_deadline_applicability() {
    let mut profile = self_employed_profile(false, None, false);
    profile.per_year_forms.insert(
        TAXABLE_YEAR,
        bir_core::forms::PerYearFormsSet::from_codes(
            TAXABLE_YEAR,
            ["1701Q"],
            bir_core::forms::FormSetSource::Manual,
        ),
    );
    let deadlines = DeadlineResolver::resolve_taxable_year(TAXABLE_YEAR as i32);
    let q1_1701q = deadlines
        .iter()
        .find(|deadline| {
            deadline.form_code == "1701Q"
                && matches!(
                    deadline.period,
                    DeadlinePeriod::Quarterly {
                        taxable_year: 2026,
                        quarter: 1
                    }
                )
        })
        .unwrap();

    assert!(deadline_applies_to_profile(&profile, q1_1701q));
}

#[test]
fn profile_version_validation_rejects_overlaps_and_ignores_draft_versions() {
    let mut profile = self_employed_profile(false, None, false);
    let current = confirmed_version(
        &profile,
        "current",
        "Current COR",
        Some((2025, 1, 1)),
        None,
        vec![
            RegisteredTaxType::IncomeTax,
            RegisteredTaxType::PercentageTax,
            RegisteredTaxType::RegistrationFee,
        ],
        false,
    );
    let mut draft_vat = confirmed_version(
        &profile,
        "draft",
        "Draft VAT COR",
        Some((2026, 1, 1)),
        None,
        vec![
            RegisteredTaxType::IncomeTax,
            RegisteredTaxType::ValueAddedTax,
            RegisteredTaxType::RegistrationFee,
        ],
        true,
    );
    draft_vat.status = TaxProfileVersionStatus::Draft;
    profile.profile_versions = vec![current.clone(), draft_vat];
    profile.compliance_source_mode = ComplianceSourceMode::CorVersioned;

    let forms = forms_for(&profile);
    assert!(forms.contains("2551Q"));
    assert!(!forms.contains("2550Q"));
    assert!(
        validate_profile(&profile)
            .iter()
            .all(|error| error.field != "profile_versions")
    );

    let mut overlapping = confirmed_version(
        &profile,
        "overlap",
        "Overlapping COR",
        Some((2026, 1, 1)),
        None,
        vec![RegisteredTaxType::IncomeTax],
        false,
    );
    overlapping.status = TaxProfileVersionStatus::Confirmed;
    profile.profile_versions = vec![current, overlapping];
    profile.compliance_source_mode = ComplianceSourceMode::CorVersioned;

    assert!(
        validate_profile(&profile)
            .iter()
            .any(|error| error.field == "profile_versions")
    );
}

#[test]
fn yearly_profile_resolution_excludes_undated_confirmed_version() {
    let mut profile = self_employed_profile(false, None, false);
    let version = confirmed_version(
        &profile,
        "undated",
        "Undated migrated profile",
        None,
        None,
        vec![RegisteredTaxType::PercentageTax],
        false,
    );
    profile.profile_versions = vec![version];
    profile.compliance_source_mode = ComplianceSourceMode::CorVersioned;

    let resolved = profile.resolve_tax_profile_for_year(TAXABLE_YEAR);

    assert!(resolved.effective_segments.is_empty());
    assert_eq!(resolved.issues.len(), 1);
}

#[test]
fn yearly_profile_resolution_accepts_sequential_midyear_versions() {
    let mut profile = self_employed_profile(false, None, false);
    let first = confirmed_version(
        &profile,
        "first",
        "First half",
        Some((2026, 1, 1)),
        Some((2026, 6, 30)),
        vec![RegisteredTaxType::PercentageTax],
        false,
    );
    let second = confirmed_version(
        &profile,
        "second",
        "Second half",
        Some((2026, 7, 1)),
        None,
        vec![RegisteredTaxType::ValueAddedTax],
        true,
    );
    profile.profile_versions = vec![second, first];
    profile.compliance_source_mode = ComplianceSourceMode::CorVersioned;

    let resolved = profile.resolve_tax_profile_for_year(TAXABLE_YEAR);

    assert_eq!(resolved.effective_segments.len(), 2);
    assert!(resolved.issues.is_empty());
}

#[test]
fn compensation_withholding_does_not_suggest_form_1600() {
    let mut profile = self_employed_profile(false, None, false);
    profile.withholds_compensation = true;
    let version = confirmed_version(
        &profile,
        "compensation",
        "Compensation withholding",
        Some((2026, 1, 1)),
        None,
        vec![RegisteredTaxType::WithholdingCompensation],
        false,
    );
    profile.profile_versions = vec![version];
    profile.compliance_source_mode = ComplianceSourceMode::CorVersioned;

    let forms = forms_for(&profile);

    assert!(!forms.contains("1600"));
}

#[test]
fn vat_percentage_withholding_tax_type_suggests_form_1600() {
    let mut profile = self_employed_profile(false, None, false);
    let version = confirmed_version(
        &profile,
        "vat-percentage-withholding",
        "VAT and percentage withholding",
        Some((2026, 1, 1)),
        None,
        vec![RegisteredTaxType::WithholdingVatAndPercentage],
        false,
    );
    profile.profile_versions = vec![version];
    profile.compliance_source_mode = ComplianceSourceMode::CorVersioned;

    let forms = forms_for(&profile);

    assert!(forms.contains("1600"));
}

#[test]
fn profile_scoped_deadline_override_applies_after_global_rules() {
    let mut profile = self_employed_profile(false, None, false);
    let mut version = confirmed_version(
        &profile,
        "cor-2026",
        "2026 COR",
        Some((2026, 1, 1)),
        None,
        vec![
            RegisteredTaxType::IncomeTax,
            RegisteredTaxType::PercentageTax,
            RegisteredTaxType::RegistrationFee,
        ],
        false,
    );
    version.deadline_overrides.push(ProfileDeadlineOverride {
        id: "profile-q1-extension".into(),
        title: "Profile-specific Q1 extension".into(),
        source_reference: "Manual COR override".into(),
        affected_form_codes: vec!["1701Q".into()],
        original_deadline: NaiveDate::from_ymd_opt(2026, 5, 15).unwrap(),
        adjusted_deadline: NaiveDate::from_ymd_opt(2026, 5, 20).unwrap(),
        reason: Some("RDO-specific extension".into()),
    });
    profile.profile_versions = vec![version];
    profile.compliance_source_mode = ComplianceSourceMode::CorVersioned;

    let overrides = profile_deadline_overrides_for_year(&profile, 2026);
    let deadlines = DeadlineResolver::resolve_taxable_year_with_overrides(2026, &overrides);
    let q1_1701q = deadlines
        .iter()
        .find(|deadline| {
            deadline.form_code == "1701Q"
                && matches!(
                    deadline.period,
                    DeadlinePeriod::Quarterly {
                        taxable_year: 2026,
                        quarter: 1
                    }
                )
        })
        .unwrap();

    assert!(matches!(
        q1_1701q.deadline,
        DeadlineKind::Dated {
            final_deadline,
            ..
        } if final_deadline == NaiveDate::from_ymd_opt(2026, 5, 20).unwrap()
    ));
}

#[test]
fn profile_global_deadline_override_conflict_is_reported() {
    let mut profile = self_employed_profile(false, None, false);
    let mut version = confirmed_version(
        &profile,
        "cor-2026",
        "2026 COR",
        Some((2026, 1, 1)),
        None,
        vec![
            RegisteredTaxType::IncomeTax,
            RegisteredTaxType::PercentageTax,
        ],
        false,
    );
    version.deadline_overrides.push(ProfileDeadlineOverride {
        id: "profile-q1-extension".into(),
        title: "Profile-specific Q1 extension".into(),
        source_reference: "Manual COR override".into(),
        affected_form_codes: vec!["1701Q".into()],
        original_deadline: NaiveDate::from_ymd_opt(2026, 5, 15).unwrap(),
        adjusted_deadline: NaiveDate::from_ymd_opt(2026, 5, 20).unwrap(),
        reason: Some("RDO-specific extension".into()),
    });
    profile.profile_versions = vec![version];
    profile.compliance_source_mode = ComplianceSourceMode::CorVersioned;

    let global_overrides = vec![DeadlineOverride {
        id: "global-q1-extension".into(),
        title: "Global Q1 extension".into(),
        source_reference: "BIR advisory".into(),
        affected_form_codes: vec!["1701Q".into()],
        original_deadline: NaiveDate::from_ymd_opt(2026, 5, 15).unwrap(),
        adjusted_deadline: NaiveDate::from_ymd_opt(2026, 5, 19).unwrap(),
        affected_regions: vec![],
        affected_taxpayer_types: vec![],
        effective_from: None,
        effective_until: None,
        expires_at: None,
    }];

    let preview = resolve_profile_obligations_for_year_with_global_overrides(
        &profile,
        2026,
        &global_overrides,
    );
    let issue = preview
        .consistency_report
        .issues
        .iter()
        .find(|issue| issue.code == "PROFILE_GLOBAL_DEADLINE_OVERRIDE_CONFLICT")
        .expect("missing profile/global override conflict diagnostic");

    assert_eq!(issue.version_id.as_deref(), Some("cor-2026"));
    assert_eq!(issue.form_code.as_deref(), Some("1701Q"));
    assert_eq!(
        issue.source.as_deref(),
        Some("Profile deadline override + global deadline override")
    );
    assert!(
        issue
            .fix_hint
            .as_deref()
            .unwrap_or_default()
            .contains("Profile-specific overrides win")
    );
}

#[test]
fn profile_override_validation_requires_reason_and_source() {
    let mut profile = self_employed_profile(false, None, false);
    let mut version = confirmed_version(
        &profile,
        "cor-2026",
        "2026 COR",
        Some((2026, 1, 1)),
        None,
        vec![
            RegisteredTaxType::IncomeTax,
            RegisteredTaxType::PercentageTax,
        ],
        false,
    );
    version.obligation_overrides.push(ManualObligationOverride {
        form_code: "2551Q".into(),
        action: ManualObligationOverrideAction::Exclude,
        reason: String::new(),
        source_reference: None,
    });
    version.deadline_overrides.push(ProfileDeadlineOverride {
        id: "missing-source".into(),
        title: String::new(),
        source_reference: String::new(),
        affected_form_codes: vec![],
        original_deadline: NaiveDate::from_ymd_opt(2026, 4, 25).unwrap(),
        adjusted_deadline: NaiveDate::from_ymd_opt(2026, 4, 28).unwrap(),
        reason: None,
    });
    profile.profile_versions = vec![version];
    profile.compliance_source_mode = ComplianceSourceMode::CorVersioned;

    let messages = validate_profile(&profile)
        .into_iter()
        .map(|error| error.message)
        .collect::<Vec<_>>();

    assert!(
        messages
            .iter()
            .any(|message| message.contains("requires a reason and source"))
    );
    assert!(
        messages
            .iter()
            .any(|message| { message.contains("requires a title, source, and form code") })
    );
}

#[test]
fn cor_consistency_report_explains_8_percent_percentage_tax_suppression() {
    let mut profile = self_employed_profile(false, None, true);
    let version = confirmed_version(
        &profile,
        "cor-2026",
        "2026 COR",
        Some((2026, 1, 1)),
        None,
        vec![
            RegisteredTaxType::IncomeTax,
            RegisteredTaxType::PercentageTax,
        ],
        false,
    );
    profile.profile_versions = vec![version];
    profile.compliance_source_mode = ComplianceSourceMode::CorVersioned;
    configure_forms_set_for_year(&mut profile, 2026);

    let preview = profile.preview_obligations_for_year(2026);
    let issue = preview
        .consistency_report
        .issues
        .iter()
        .find(|issue| issue.code == "PERCENTAGE_TAX_SUPPRESSED_BY_8_PERCENT")
        .expect("8% suppression diagnostic");

    assert_eq!(
        issue.severity,
        bir_core::integration::ProfileConsistencySeverity::Info
    );
    assert_eq!(issue.version_id.as_deref(), Some("cor-2026"));
    assert_eq!(issue.source.as_deref(), Some("Income tax election + TTCE"));
    assert!(
        issue
            .fix_hint
            .as_deref()
            .unwrap_or_default()
            .contains("income tax election ledger")
    );
    assert!(!preview.form_codes.iter().any(|code| code == "2551Q"));
}

#[test]
fn eight_percent_election_preserves_non_pt010_percentage_tax_obligation() {
    let mut profile = self_employed_profile(false, None, true);
    profile.atc_codes = vec!["PT010".into(), "PT040".into()];
    let version = confirmed_version(
        &profile,
        "cor-2026",
        "2026 COR",
        Some((2026, 1, 1)),
        None,
        vec![
            RegisteredTaxType::IncomeTax,
            RegisteredTaxType::PercentageTax,
        ],
        false,
    );
    profile.profile_versions = vec![version];
    profile.compliance_source_mode = ComplianceSourceMode::CorVersioned;
    configure_forms_set_for_year(&mut profile, 2026);

    let preview = profile.preview_obligations_for_year(2026);

    assert!(preview.form_codes.iter().any(|code| code == "2551Q"));
    assert!(
        preview
            .consistency_report
            .issues
            .iter()
            .all(|issue| issue.code != "PERCENTAGE_TAX_SUPPRESSED_BY_8_PERCENT")
    );
}

#[test]
fn cor_consistency_report_flags_cor_tin_mismatch() {
    let mut profile = self_employed_profile(false, None, false);
    let mut version = confirmed_version(
        &profile,
        "cor-2026",
        "2026 COR",
        Some((2026, 1, 1)),
        None,
        vec![
            RegisteredTaxType::IncomeTax,
            RegisteredTaxType::PercentageTax,
        ],
        false,
    );
    version.cor.tin = Some("999-888-777-00000".into());
    profile.profile_versions = vec![version];
    profile.compliance_source_mode = ComplianceSourceMode::CorVersioned;
    configure_forms_set_for_year(&mut profile, 2026);

    let preview = profile.preview_obligations_for_year(2026);
    let issue = preview
        .consistency_report
        .issues
        .iter()
        .find(|issue| issue.code == "COR_TIN_MISMATCH")
        .expect("COR TIN mismatch diagnostic");

    assert_eq!(
        issue.severity,
        bir_core::integration::ProfileConsistencySeverity::NeedsReview
    );
    assert_eq!(issue.version_id.as_deref(), Some("cor-2026"));
    assert_eq!(issue.source.as_deref(), Some("COR/profile version"));
    assert!(
        issue
            .fix_hint
            .as_deref()
            .unwrap_or_default()
            .contains("Verify the uploaded COR")
    );
}

#[test]
fn cor_consistency_report_includes_manual_override_context() {
    let mut profile = self_employed_profile(false, None, false);
    let mut version = confirmed_version(
        &profile,
        "cor-2026",
        "2026 COR",
        Some((2026, 1, 1)),
        None,
        vec![RegisteredTaxType::IncomeTax],
        false,
    );
    version.obligation_overrides.push(ManualObligationOverride {
        form_code: "9999Z".into(),
        action: ManualObligationOverrideAction::Include,
        reason: "Local exception".into(),
        source_reference: Some("RDO memo".into()),
    });
    profile.profile_versions = vec![version];
    profile.compliance_source_mode = ComplianceSourceMode::CorVersioned;
    configure_forms_set_for_year(&mut profile, 2026);

    let preview = profile.preview_obligations_for_year(2026);
    let issue = preview
        .consistency_report
        .issues
        .iter()
        .find(|issue| issue.code == "MANUAL_INCLUDE_UNKNOWN_FORM")
        .expect("manual include diagnostic");

    assert_eq!(issue.version_id.as_deref(), Some("cor-2026"));
    assert_eq!(issue.form_code.as_deref(), Some("9999Z"));
    assert_eq!(issue.source.as_deref(), Some("Manual obligation override"));
    assert!(
        issue
            .fix_hint
            .as_deref()
            .unwrap_or_default()
            .contains("TTCE/calendar support")
    );
}

#[test]
fn abolished_1704_is_not_a_current_corporate_dashboard_obligation() {
    let mut profile = base_profile(
        TaxpayerType::Corporation,
        Some(TaxClassification::Corporation),
    );
    let version = confirmed_version(
        &profile,
        "corp-cor-2026",
        "Corporate COR",
        Some((2026, 1, 1)),
        None,
        vec![RegisteredTaxType::IncomeTax],
        false,
    );
    profile.profile_versions = vec![version];
    profile.compliance_source_mode = ComplianceSourceMode::CorVersioned;
    configure_forms_set_for_year(&mut profile, 2026);

    let preview = profile.preview_obligations_for_year(2026);
    assert!(!preview.form_codes.iter().any(|code| code == "1704"));
    assert!(
        !preview
            .consistency_report
            .issues
            .iter()
            .any(|issue| issue.code == "OBLIGATION_WITHOUT_CALENDAR_RULE"
                && issue.form_code.as_deref() == Some("1704")),
        "abolished 1704 should not produce a missing calendar rule diagnostic for 2026"
    );
}

#[test]
fn manual_include_still_reports_missing_calendar_rule_for_1704() {
    let mut profile = base_profile(
        TaxpayerType::Corporation,
        Some(TaxClassification::Corporation),
    );
    let mut version = confirmed_version(
        &profile,
        "corp-cor-2026",
        "Corporate COR",
        Some((2026, 1, 1)),
        None,
        vec![RegisteredTaxType::IncomeTax],
        false,
    );
    version.obligation_overrides.push(ManualObligationOverride {
        form_code: "1704".into(),
        action: ManualObligationOverrideAction::Include,
        reason: "Legacy/special IAET obligation asserted by user".into(),
        source_reference: Some("Manual evidence".into()),
    });
    profile.profile_versions = vec![version];
    profile.compliance_source_mode = ComplianceSourceMode::CorVersioned;
    configure_forms_set_for_year(&mut profile, 2026);

    let preview = profile.preview_obligations_for_year(2026);
    let issue = preview
        .consistency_report
        .issues
        .iter()
        .find(|issue| {
            issue.code == "OBLIGATION_WITHOUT_CALENDAR_RULE"
                && issue.form_code.as_deref() == Some("1704")
        })
        .expect("missing calendar rule diagnostic for 1704");

    assert_eq!(issue.version_id.as_deref(), Some("corp-cor-2026"));
    assert_eq!(issue.source.as_deref(), Some("Calendar rules"));
    assert!(
        issue
            .fix_hint
            .as_deref()
            .unwrap_or_default()
            .contains("official calendar rule")
    );
    assert!(preview.form_codes.iter().any(|code| code == "1704"));
}

#[test]
fn profile_version_validation_rejects_invalid_effective_date_range() {
    let mut profile = self_employed_profile(false, None, false);
    let version = confirmed_version(
        &profile,
        "bad-range",
        "Bad Range COR",
        Some((2026, 6, 1)),
        Some((2026, 5, 31)),
        vec![RegisteredTaxType::IncomeTax],
        false,
    );
    profile.profile_versions = vec![version];
    profile.compliance_source_mode = ComplianceSourceMode::CorVersioned;

    assert!(validate_profile(&profile).iter().any(|error| {
        error
            .message
            .contains("effective end date before its start date")
    }));
}

#[test]
fn profile_tin_must_pass_the_official_check_digit() {
    let mut profile = self_employed_profile(false, None, false);
    assert!(
        validate_profile(&profile)
            .iter()
            .all(|error| error.field != "tin")
    );

    // 010-558-055 fails chkt.exe's check digit.
    profile.tin.segment3 = "055".into();
    assert!(validate_profile(&profile).iter().any(|error| {
        error.field == "tin" && error.message == "You have entered an incorrect TIN"
    }));

    // Dummy TINs are rejected even though chkt.exe accepts 000000000.
    profile.tin.segment1 = "000".into();
    profile.tin.segment2 = "000".into();
    profile.tin.segment3 = "000".into();
    assert!(
        validate_profile(&profile)
            .iter()
            .any(|error| error.field == "tin")
    );
}
