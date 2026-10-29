use bir_core::integration::{
    form_suggestions_for_profile_year, recurring_obligation_forms_for_profile_and_year,
};
use bir_core::naming::Tin;
use bir_core::profile::{
    ComplianceSourceMode, RegisteredTaxType, TaxClassification, TaxProfileVersion,
    TaxProfileVersionStatus, TaxpayerProfile, TaxpayerType,
};

use chrono::NaiveDate;

fn base_profile(
    taxpayer_type: TaxpayerType,
    classification: Option<TaxClassification>,
) -> TaxpayerProfile {
    TaxpayerProfile {
        id: Some(1),
        full_name: "Adversarial Stress Test Taxpayer".into(),
        tin: Tin {
            segment1: "123".into(),
            segment2: "456".into(),
            segment3: "789".into(),
            branch: "000".into(),
        },
        rdo_code: "039".into(),
        line_of_business: "Testing".into(),
        registered_address: "Manila".into(),
        zip_code: "1000".into(),
        phone: "09123456789".into(),
        email: "stress@example.com".into(),
        default_form_type: "1701".into(),
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
        totp_secret: None,
        email_tracking_enabled: false,
        email_auth_method: Default::default(),
        imap_email: None,
        imap_host: None,
        test_notification_enabled: false,
        imap_app_password: None,
        oauth_access_token: None,
        oauth_refresh_token: None,
        profile_versions: vec![],
        birth_date: None,
        compliance_source_mode: ComplianceSourceMode::CorVersioned,
        per_year_forms: Default::default(),
    }
}

fn confirmed_version(
    profile: &TaxpayerProfile,
    id: &str,
    from: Option<NaiveDate>,
    until: Option<NaiveDate>,
    tax_types: Vec<RegisteredTaxType>,
    vat: bool,
) -> TaxProfileVersion {
    let mut version = TaxProfileVersion::from_profile_backfill(profile);
    version.id = id.to_string();
    version.label = format!("Version {id}");
    version.status = TaxProfileVersionStatus::Confirmed;
    version.effective_from = from;
    version.effective_until = until;
    version.needs_effective_date_review = false;
    version.registered_tax_types = tax_types;
    version.is_vat_registered = vat;
    version
}

fn reconcile_forms_set(profile: &mut TaxpayerProfile, year: u16) {
    let suggestions = form_suggestions_for_profile_year(profile, year);
    let existing = profile.per_year_forms.get(&year);
    let reconciled = bir_core::forms::reconcile_forms_set_for_year(year, existing, &suggestions);
    assert!(
        reconciled.conflicts.is_empty(),
        "stress fixture unexpectedly produced Forms Set conflicts: {:?}",
        reconciled.conflicts
    );
    profile.per_year_forms.insert(year, reconciled.forms_set);
}

#[test]
fn test_adversarial_individual_vs_corporate_leakage() {
    let mut profile = base_profile(
        TaxpayerType::Individual,
        Some(TaxClassification::SelfEmployed),
    );
    let version = confirmed_version(
        &profile,
        "v-indiv",
        Some(NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
        None,
        vec![
            RegisteredTaxType::IncomeTax,
            RegisteredTaxType::PercentageTax,
            RegisteredTaxType::RegistrationFee,
        ],
        false,
    );
    profile.profile_versions = vec![version];
    reconcile_forms_set(&mut profile, 2026);

    let forms = recurring_obligation_forms_for_profile_and_year(&profile, 2026);

    // Assert Individual forms are suggested, and corporate forms are NOT suggested
    assert!(forms.contains(&"1701".to_string()), "Should contain 1701");
    assert!(forms.contains(&"1701Q".to_string()), "Should contain 1701Q");

    // Corporate forms: 1702RT, 1702EX, 1702MX, 1702Q
    for f in &["1702", "1702RT", "1702EX", "1702MX", "1702Q", "1704"] {
        assert!(
            !forms.contains(&f.to_string()),
            "Individual profile leaked corporate form: {f}"
        );
    }

    // Conversely, Corporate profile
    let mut corp_profile = base_profile(TaxpayerType::Corporation, None);
    let corp_version = confirmed_version(
        &corp_profile,
        "v-corp",
        Some(NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
        None,
        vec![
            RegisteredTaxType::IncomeTax,
            RegisteredTaxType::PercentageTax,
            RegisteredTaxType::RegistrationFee,
        ],
        false,
    );
    corp_profile.profile_versions = vec![corp_version];
    reconcile_forms_set(&mut corp_profile, 2026);

    let corp_forms = recurring_obligation_forms_for_profile_and_year(&corp_profile, 2026);
    assert!(
        corp_forms.contains(&"1702RT".to_string()),
        "Should contain 1702RT"
    );
    assert!(
        corp_forms.contains(&"1702Q".to_string()),
        "Should contain 1702Q"
    );

    // Individual forms: 1700, 1701, 1701A, 1701Q, 1701MS
    for f in &["1700", "1701", "1701A", "1701Q", "1701MS"] {
        assert!(
            !corp_forms.contains(&f.to_string()),
            "Corporate profile leaked individual form: {f}"
        );
    }
}

#[test]
fn test_adversarial_vat_vs_non_vat_leakage() {
    // 1. Non-VAT profile
    let mut non_vat_profile = base_profile(
        TaxpayerType::Individual,
        Some(TaxClassification::SelfEmployed),
    );
    let non_vat_version = confirmed_version(
        &non_vat_profile,
        "v-non-vat",
        Some(NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
        None,
        vec![
            RegisteredTaxType::IncomeTax,
            RegisteredTaxType::PercentageTax,
            RegisteredTaxType::RegistrationFee,
        ],
        false,
    );
    non_vat_profile.profile_versions = vec![non_vat_version];
    reconcile_forms_set(&mut non_vat_profile, 2026);

    let non_vat_forms = recurring_obligation_forms_for_profile_and_year(&non_vat_profile, 2026);
    assert!(
        non_vat_forms.contains(&"2551Q".to_string()),
        "Should suggest Percentage Tax 2551Q"
    );
    assert!(
        !non_vat_forms.contains(&"2550Q".to_string()),
        "Should NOT suggest VAT 2550Q"
    );
    assert!(
        !non_vat_forms.contains(&"2550M".to_string()),
        "Should NOT suggest VAT 2550M"
    );

    // 2. VAT profile
    let mut vat_profile = base_profile(
        TaxpayerType::Individual,
        Some(TaxClassification::SelfEmployed),
    );
    let vat_version = confirmed_version(
        &vat_profile,
        "v-vat",
        Some(NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
        None,
        vec![
            RegisteredTaxType::IncomeTax,
            RegisteredTaxType::ValueAddedTax,
            RegisteredTaxType::RegistrationFee,
        ],
        true,
    );
    vat_profile.profile_versions = vec![vat_version];
    vat_profile.is_vat_registered = true;
    reconcile_forms_set(&mut vat_profile, 2026);

    let vat_forms = recurring_obligation_forms_for_profile_and_year(&vat_profile, 2026);
    assert!(
        vat_forms.contains(&"2550Q".to_string()),
        "Should suggest VAT 2550Q"
    );
    assert!(
        !vat_forms.contains(&"2550M".to_string()),
        "Should not suggest deprecated monthly VAT form 2550M in 2026"
    );
    assert!(
        !vat_forms.contains(&"2551Q".to_string()),
        "Should NOT suggest Percentage Tax 2551Q"
    );
}

#[test]
fn test_adversarial_mid_year_transition_union() {
    let mut profile = base_profile(
        TaxpayerType::Individual,
        Some(TaxClassification::SelfEmployed),
    );

    // Version 1: Jan 1 to June 30, Non-VAT (Percentage Tax registered)
    let v1 = confirmed_version(
        &profile,
        "v1-non-vat",
        Some(NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
        Some(NaiveDate::from_ymd_opt(2026, 6, 30).unwrap()),
        vec![
            RegisteredTaxType::IncomeTax,
            RegisteredTaxType::PercentageTax,
            RegisteredTaxType::RegistrationFee,
        ],
        false,
    );

    // Version 2: July 1 onwards, VAT-registered
    let v2 = confirmed_version(
        &profile,
        "v2-vat",
        Some(NaiveDate::from_ymd_opt(2026, 7, 1).unwrap()),
        None,
        vec![
            RegisteredTaxType::IncomeTax,
            RegisteredTaxType::ValueAddedTax,
            RegisteredTaxType::RegistrationFee,
        ],
        true,
    );

    profile.profile_versions = vec![v1, v2];
    reconcile_forms_set(&mut profile, 2026);

    let forms = recurring_obligation_forms_for_profile_and_year(&profile, 2026);

    // Resolved forms for 2026 must contain both percentage tax and quarterly VAT
    // obligations so no historical filing obligations are lost mid-year.
    assert!(
        forms.contains(&"2551Q".to_string()),
        "Union should preserve 2551Q from first half of year"
    );
    assert!(
        forms.contains(&"2550Q".to_string()),
        "Union should include 2550Q from second half of year"
    );
    assert!(
        !forms.contains(&"2550M".to_string()),
        "Union should exclude deprecated monthly VAT form 2550M in 2026"
    );
}

#[test]
fn test_adversarial_year_aware_deprecations() {
    let mut profile = base_profile(TaxpayerType::Corporation, None);

    // Add withholding tax types so withholding forms are checked
    let mut version_2017 = confirmed_version(
        &profile,
        "v-2017",
        Some(NaiveDate::from_ymd_opt(2017, 1, 1).unwrap()),
        None,
        vec![
            RegisteredTaxType::IncomeTax,
            RegisteredTaxType::PercentageTax,
            RegisteredTaxType::WithholdingExpanded,
        ],
        false,
    );
    version_2017.withholds_expanded = true;

    profile.profile_versions = vec![version_2017];
    profile.withholds_expanded = true;
    profile.is_expanded_withholding_agent = true;
    reconcile_forms_set(&mut profile, 2017);
    reconcile_forms_set(&mut profile, 2018);
    reconcile_forms_set(&mut profile, 2021);

    // 1. Year 2017 (before deprecations of 1601E and 2551M, and 1704)
    let forms_2017 = recurring_obligation_forms_for_profile_and_year(&profile, 2017);
    assert!(
        forms_2017.contains(&"1601E".to_string()),
        "1601E should be active in 2017"
    );
    assert!(
        forms_2017.contains(&"2551M".to_string()),
        "2551M should be active in 2017"
    );
    assert!(
        forms_2017.contains(&"1704".to_string()),
        "1704 should be active in 2017"
    );

    // 2. Year 2018 (after deprecations of 1601E and 2551M, but before 1704)
    let forms_2018 = recurring_obligation_forms_for_profile_and_year(&profile, 2018);
    assert!(
        !forms_2018.contains(&"1601E".to_string()),
        "1601E should be deprecated in 2018"
    );
    assert!(
        !forms_2018.contains(&"2551M".to_string()),
        "2551M should be deprecated in 2018"
    );
    assert!(
        forms_2018.contains(&"1704".to_string()),
        "1704 should still be active in 2018"
    );

    // 3. Year 2021 (after deprecation of 1704)
    let forms_2021 = recurring_obligation_forms_for_profile_and_year(&profile, 2021);
    assert!(
        !forms_2021.contains(&"1601E".to_string()),
        "1601E should be deprecated in 2021"
    );
    assert!(
        !forms_2021.contains(&"2551M".to_string()),
        "2551M should be deprecated in 2021"
    );
    assert!(
        !forms_2021.contains(&"1704".to_string()),
        "1704 should be deprecated in 2021"
    );
}
