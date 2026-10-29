use bir_core::db::Database;
use bir_core::forms::forms_set::{FormSetEntry, FormSetSource, PerYearFormsSet};
use bir_core::integration::resolve_profile_obligations_for_year;
use bir_core::naming::Tin;
use bir_core::profile::{
    ComplianceSourceMode, CorDocumentRef, CorRegistrationFacts, ManualObligationOverride,
    ManualObligationOverrideAction, RegisteredTaxType, TaxClassification, TaxProfileVersion,
    TaxProfileVersionSource, TaxProfileVersionStatus, TaxpayerProfile, TaxpayerType,
};
use chrono::{Datelike, NaiveDate};
use tempfile::NamedTempFile;

fn parse_tin(tin_str: &str) -> Tin {
    assert!(tin_str.len() == 12 || tin_str.len() == 14);
    Tin {
        segment1: tin_str[0..3].to_string(),
        segment2: tin_str[3..6].to_string(),
        segment3: tin_str[6..9].to_string(),
        branch: tin_str[9..].to_string(),
    }
}

fn create_test_profile(tin_str: &str) -> TaxpayerProfile {
    let tin = parse_tin(tin_str);
    let mut profile = TaxpayerProfile {
        id: None,
        full_name: "Test Taxpayer".to_string(),
        tin,
        rdo_code: "039".to_string(),
        line_of_business: "Testing Services".to_string(),
        registered_address: "123 Test St, Manila".to_string(),
        zip_code: "1000".to_string(),
        phone: "09123456789".to_string(),
        email: "test@example.com".to_string(),
        default_form_type: "1701".to_string(),
        taxpayer_type: TaxpayerType::Individual,
        is_vat_registered: false,
        business_start_date: Some(NaiveDate::from_ymd_opt(2025, 1, 1).unwrap()),
        birth_date: None,
        tax_classification: Some(TaxClassification::SelfEmployed),
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
        compliance_source_mode: ComplianceSourceMode::CorVersioned,
        per_year_forms: Default::default(),
    };
    profile.ensure_profile_version_ledger();
    profile
}

fn persisted_forms_sets(db: &Database, tin: &str) -> Vec<(u16, PerYearFormsSet)> {
    db.list_forms_set_years(tin)
        .expect("Forms Set year lookup")
        .into_iter()
        .map(|year| {
            let set = db.get_per_year_forms(tin, year).expect("Forms Set lookup");
            (year, set)
        })
        .collect()
}

fn current_local_reconciliation_year() -> u16 {
    chrono::Local::now().year() as u16
}

#[test]
fn closest_prior_forms_year_returns_latest_active_unconfigured_year() {
    let mut profile = create_test_profile("000234564000");
    profile.per_year_forms.insert(
        2023,
        PerYearFormsSet::from_codes(2023, ["1701"], FormSetSource::Manual),
    );
    profile.per_year_forms.insert(
        2025,
        PerYearFormsSet::from_codes(2025, ["1701Q"], FormSetSource::Manual),
    );

    assert_eq!(profile.closest_prior_forms_year(2026), Some(2025));
}

#[test]
fn closest_prior_forms_year_is_hidden_when_destination_has_entries() {
    let mut profile = create_test_profile("000234564000");
    profile.per_year_forms.insert(
        2025,
        PerYearFormsSet::from_codes(2025, ["1701"], FormSetSource::Manual),
    );
    let mut destination = PerYearFormsSet::from_codes(2026, ["1701Q"], FormSetSource::Manual);
    destination.entries[0].active = false;
    profile.per_year_forms.insert(2026, destination);

    assert_eq!(profile.closest_prior_forms_year(2026), None);
}

#[test]
fn profile_save_preserves_existing_atc_codes_when_editor_omits_them() {
    temp_env::with_var("EBIR_TEST_ENV", Some("1"), || {
        let temp_file = NamedTempFile::new().unwrap();
        let db = Database::open(temp_file.path()).expect("Failed to open DB");
        let mut profile = create_test_profile("000234564000");
        profile.atc_codes = vec!["PT010".into(), "PT040".into()];
        let saved = db.save_profile(profile).expect("initial profile save");

        let mut editor_payload = saved;
        editor_payload.atc_codes.clear();
        db.save_profile(editor_payload).expect("profile edit save");

        let reloaded = db
            .get_profile("000234564000")
            .expect("profile lookup")
            .expect("stored profile");
        assert_eq!(reloaded.atc_codes, vec!["PT010", "PT040"]);
    });
}

#[test]
fn profile_save_reconciles_generated_forms_and_preserves_manual_entry() {
    temp_env::with_var("EBIR_TEST_ENV", Some("1"), || {
        let temp_file = NamedTempFile::new().unwrap();
        let db = Database::open(temp_file.path()).expect("Failed to open DB");
        let mut profile = create_test_profile("000234564000");
        profile.profile_versions[0].id = "confirmed-cor".into();
        profile.profile_versions[0].source = TaxProfileVersionSource::ManualCor;
        profile.profile_versions[0].registered_tax_types = vec![
            RegisteredTaxType::IncomeTax,
            RegisteredTaxType::PercentageTax,
        ];
        let saved = save_initial_confirmed_profile(&db, profile);
        let year = current_local_reconciliation_year();

        let mut set = saved
            .per_year_forms
            .get(&year)
            .cloned()
            .expect("generated current-year Forms Set");
        set.entries.push(FormSetEntry::from_code(
            "CUSTOM_FORM",
            FormSetSource::Manual,
        ));
        db.save_per_year_forms(&saved.tin.full(), year, &set)
            .expect("manual forms save");

        let mut changed = db
            .get_profile(&saved.tin.full())
            .expect("profile lookup")
            .expect("stored profile");
        let mut replacement = changed.profile_versions[0].clone();
        replacement.id = "confirmed-vat-cor".to_string();
        replacement.label = "Confirmed VAT COR".to_string();
        replacement.status = TaxProfileVersionStatus::Draft;
        replacement.effective_from = NaiveDate::from_ymd_opt(year as i32, 1, 1);
        replacement.effective_until = None;
        replacement.is_vat_registered = true;
        replacement.registered_tax_types = vec![
            RegisteredTaxType::IncomeTax,
            RegisteredTaxType::ValueAddedTax,
        ];
        changed.profile_versions.push(replacement);
        let mut changed = db
            .save_profile(changed)
            .expect("VAT replacement draft save");
        let effective_from = NaiveDate::from_ymd_opt(year as i32, 1, 1).unwrap();
        let plan = changed
            .profile_version_confirmation_plan("confirmed-vat-cor", effective_from)
            .expect("VAT replacement confirmation plan");
        assert!(changed.apply_profile_version_confirmation_plan(&plan));
        let changed = db
            .save_profile_with_confirmation_plan(changed, &plan)
            .expect("reviewed VAT profile save");
        let reconciled = changed
            .per_year_forms
            .get(&year)
            .expect("reconciled Forms Set");

        assert!(reconciled.contains_active("CUSTOM_FORM"));
        assert!(reconciled.contains_active("2550Q"));
        assert!(!reconciled.contains_active("2551Q"));
        assert!(reconciled.entry("2551Q").is_some());
    });
}

#[test]
fn confirmed_profile_change_reconciles_current_and_stored_intersecting_years() {
    temp_env::with_var("EBIR_TEST_ENV", Some("1"), || {
        let temp_file = NamedTempFile::new().unwrap();
        let db = Database::open(temp_file.path()).expect("Failed to open DB");
        let current_year = current_local_reconciliation_year();
        let mid_year = current_year - 1;
        let historical_year = current_year - 2;

        let mut profile = create_test_profile("000234564000");
        let historical_start = NaiveDate::from_ymd_opt(i32::from(historical_year), 1, 1).unwrap();
        profile.business_start_date = Some(historical_start);
        profile.profile_versions[0].id = "percentage-tax-cor".into();
        profile.profile_versions[0].source = TaxProfileVersionSource::ManualCor;
        profile.profile_versions[0].effective_from = Some(historical_start);
        profile.profile_versions[0].cor.registration_date = Some(historical_start);
        profile.profile_versions[0].registered_tax_types = vec![
            RegisteredTaxType::IncomeTax,
            RegisteredTaxType::PercentageTax,
        ];
        let saved = save_initial_confirmed_profile(&db, profile);
        let tin = saved.tin.full();

        let mut historical_manual = PerYearFormsSet::from_codes(
            historical_year,
            ["CUSTOM_HISTORICAL", "2551Q"],
            FormSetSource::Manual,
        );
        let suppressed_historical_percentage_tax = historical_manual
            .entries
            .iter_mut()
            .find(|entry| entry.form_code == "2551Q")
            .expect("historical manual 2551Q entry");
        suppressed_historical_percentage_tax.active = false;
        suppressed_historical_percentage_tax.reason =
            Some("Historical suppression by the taxpayer".into());
        db.save_per_year_forms(&tin, historical_year, &historical_manual)
            .expect("historical Forms Set save");

        let mid_year_manual =
            PerYearFormsSet::from_codes(mid_year, ["CUSTOM_MID_YEAR"], FormSetSource::Manual);
        db.save_per_year_forms(&tin, mid_year, &mid_year_manual)
            .expect("mid-year Forms Set save");

        let mut current_set = db
            .get_per_year_forms(&tin, current_year)
            .expect("generated current-year Forms Set");
        current_set.entries.push(FormSetEntry::from_code(
            "CUSTOM_CURRENT",
            FormSetSource::Manual,
        ));
        db.save_per_year_forms(&tin, current_year, &current_set)
            .expect("current-year Forms Set save");

        let mut changed = db
            .get_profile(&tin)
            .expect("profile lookup")
            .expect("stored profile");
        let mut vat_replacement = changed.profile_versions[0].clone();
        vat_replacement.id = "mid-year-vat-cor".into();
        vat_replacement.label = "Mid-year VAT COR".into();
        vat_replacement.status = TaxProfileVersionStatus::Draft;
        vat_replacement.effective_from = NaiveDate::from_ymd_opt(i32::from(historical_year), 7, 1);
        vat_replacement.effective_until = None;
        vat_replacement.is_vat_registered = true;
        vat_replacement.registered_tax_types = vec![
            RegisteredTaxType::IncomeTax,
            RegisteredTaxType::ValueAddedTax,
        ];
        changed.profile_versions.push(vat_replacement);
        let mut changed = db
            .save_profile(changed)
            .expect("VAT replacement draft save");
        let effective_from = NaiveDate::from_ymd_opt(i32::from(historical_year), 7, 1).unwrap();
        let plan = changed
            .profile_version_confirmation_plan("mid-year-vat-cor", effective_from)
            .expect("VAT replacement confirmation plan");
        assert!(changed.apply_profile_version_confirmation_plan(&plan));
        db.save_profile_with_confirmation_plan(changed, &plan)
            .expect("reviewed VAT profile save");

        let historical = db
            .get_per_year_forms(&tin, historical_year)
            .expect("reconciled historical Forms Set");
        let mid_year_set = db
            .get_per_year_forms(&tin, mid_year)
            .expect("reconciled mid-year Forms Set");
        let current = db
            .get_per_year_forms(&tin, current_year)
            .expect("reconciled current-year Forms Set");
        let entry_state = |set: &PerYearFormsSet, code: &str| {
            set.entry(code).map(|entry| (entry.active, entry.source))
        };
        let actual = (
            db.list_forms_set_years(&tin)
                .expect("stored Forms Set years"),
            entry_state(&historical, "CUSTOM_HISTORICAL"),
            entry_state(&historical, "2551Q"),
            entry_state(&historical, "2550Q"),
            historical
                .entry("2551Q")
                .and_then(|entry| entry.reason.clone()),
            entry_state(&mid_year_set, "CUSTOM_MID_YEAR"),
            entry_state(&mid_year_set, "2551Q"),
            entry_state(&mid_year_set, "2550Q"),
            entry_state(&current, "CUSTOM_CURRENT"),
            entry_state(&current, "2551Q"),
            entry_state(&current, "2550Q"),
        );
        let expected = (
            vec![current_year, mid_year, historical_year],
            Some((true, FormSetSource::Manual)),
            Some((false, FormSetSource::Manual)),
            Some((true, FormSetSource::InferredTaxType)),
            Some("Historical suppression by the taxpayer".into()),
            Some((true, FormSetSource::Manual)),
            Some((false, FormSetSource::InferredTaxType)),
            Some((true, FormSetSource::InferredTaxType)),
            Some((true, FormSetSource::Manual)),
            Some((false, FormSetSource::InferredTaxType)),
            Some((true, FormSetSource::InferredTaxType)),
        );

        assert_eq!(actual, expected);
    });
}

#[test]
fn forms_set_reconcile_failure_rolls_back_profile_timeline_and_forms_sets() {
    temp_env::with_var("EBIR_TEST_ENV", Some("1"), || {
        let temp_file = NamedTempFile::new().unwrap();
        let db = Database::open(temp_file.path()).expect("Failed to open DB");
        let current_year = current_local_reconciliation_year();

        let mut profile = create_test_profile("000234564000");
        profile.profile_versions[0].id = "percentage-tax-cor".into();
        profile.profile_versions[0].source = TaxProfileVersionSource::ManualCor;
        profile.profile_versions[0].effective_from =
            NaiveDate::from_ymd_opt(i32::from(current_year - 1), 1, 1);
        profile.profile_versions[0].registered_tax_types = vec![
            RegisteredTaxType::IncomeTax,
            RegisteredTaxType::PercentageTax,
        ];
        let saved = save_initial_confirmed_profile(&db, profile);
        let tin = saved.tin.full();

        let mut current_set = db
            .get_per_year_forms(&tin, current_year)
            .expect("generated current-year Forms Set");
        current_set.entries.push(FormSetEntry::from_code(
            "CUSTOM_KEEP",
            FormSetSource::Manual,
        ));
        db.save_per_year_forms(&tin, current_year, &current_set)
            .expect("manual Forms Set save");

        let mut changed = db
            .get_profile(&tin)
            .expect("profile lookup")
            .expect("stored profile");
        let mut vat_replacement = changed.profile_versions[0].clone();
        vat_replacement.id = "vat-cor".into();
        vat_replacement.label = "VAT COR".into();
        vat_replacement.status = TaxProfileVersionStatus::Draft;
        vat_replacement.effective_from = NaiveDate::from_ymd_opt(i32::from(current_year), 1, 1);
        vat_replacement.effective_until = None;
        vat_replacement.is_vat_registered = true;
        vat_replacement.registered_tax_types = vec![
            RegisteredTaxType::IncomeTax,
            RegisteredTaxType::ValueAddedTax,
        ];
        changed.profile_versions.push(vat_replacement);
        let before_profile = db
            .save_profile(changed)
            .expect("VAT replacement draft save");
        let before_forms_sets = persisted_forms_sets(&db, &tin);
        let before_profile_json =
            serde_json::to_value(&before_profile).expect("profile snapshot serialization");

        let effective_from = NaiveDate::from_ymd_opt(i32::from(current_year), 1, 1).unwrap();
        let plan = before_profile
            .profile_version_confirmation_plan("vat-cor", effective_from)
            .expect("VAT replacement confirmation plan");
        let mut submitted = before_profile;
        assert!(submitted.apply_profile_version_confirmation_plan(&plan));

        let key = Database::get_or_create_master_key().expect("test database key");
        let trigger_connection =
            rusqlite::Connection::open(temp_file.path()).expect("trigger connection");
        trigger_connection
            .execute_batch(&format!(
                "PRAGMA key = \"x'{key}'\";
                 CREATE TRIGGER fail_new_vat_form_write
                 BEFORE INSERT ON per_year_forms
                 WHEN NEW.form_code = '2550Q'
                 BEGIN
                     SELECT RAISE(ABORT, 'forced Forms Set reconciliation failure');
                 END;"
            ))
            .expect("install deterministic Forms Set failure trigger");

        let error = db
            .save_profile_with_confirmation_plan(submitted, &plan)
            .expect_err("the forced Forms Set write must fail the profile save");
        assert!(
            error
                .to_string()
                .contains("forced Forms Set reconciliation failure"),
            "unexpected save error: {error}"
        );

        let after_profile = db
            .get_profile(&tin)
            .expect("profile lookup after rollback")
            .expect("stored profile after rollback");
        let after_profile_json =
            serde_json::to_value(after_profile).expect("profile snapshot serialization");
        let after_forms_sets = persisted_forms_sets(&db, &tin);

        assert_eq!(
            (after_profile_json, after_forms_sets),
            (before_profile_json, before_forms_sets)
        );
    });
}

#[test]
fn undated_migration_backfill_preserves_existing_forms_set() {
    temp_env::with_var("EBIR_TEST_ENV", Some("1"), || {
        let temp_file = NamedTempFile::new().unwrap();
        let db = Database::open(temp_file.path()).expect("Failed to open DB");
        let mut profile = create_test_profile("000234564000");
        profile.business_start_date = None;
        profile.profile_versions.clear();
        profile.ensure_profile_version_ledger();
        let saved = db.save_profile(profile).expect("undated profile save");
        let year = current_local_reconciliation_year();
        let manual = PerYearFormsSet::from_codes(year, ["1701"], FormSetSource::Manual);
        db.save_per_year_forms(&saved.tin.full(), year, &manual)
            .expect("manual Forms Set save");

        let profile = db
            .get_profile(&saved.tin.full())
            .expect("profile lookup")
            .expect("stored profile");
        let saved_again = db.save_profile(profile).expect("repeat profile save");

        assert_eq!(
            saved_again
                .per_year_forms
                .get(&year)
                .expect("preserved Forms Set")
                .active_form_codes(),
            vec!["1701"]
        );
    });
}

#[test]
fn profile_save_rejects_overlapping_confirmed_versions() {
    temp_env::with_var("EBIR_TEST_ENV", Some("1"), || {
        let temp_file = NamedTempFile::new().unwrap();
        let db = Database::open(temp_file.path()).expect("Failed to open DB");
        let mut profile = create_test_profile("000234564000");
        profile.profile_versions[0].id = "first".to_string();
        profile.profile_versions[0].source = TaxProfileVersionSource::ManualCor;
        let saved = save_initial_confirmed_profile(&db, profile);

        let mut overlapping = TaxProfileVersion::from_profile_backfill(&saved);
        overlapping.id = "overlapping".into();
        overlapping.status = TaxProfileVersionStatus::Draft;
        overlapping.source = TaxProfileVersionSource::ManualCor;
        overlapping.effective_from = NaiveDate::from_ymd_opt(2024, 6, 1);
        overlapping.effective_until = None;
        let mut changed = saved;
        changed.profile_versions.push(overlapping);
        let plan = changed
            .profile_version_confirmation_plan(
                "overlapping",
                NaiveDate::from_ymd_opt(2024, 6, 1).unwrap(),
            )
            .expect("confirmation plan should be available");
        assert!(changed.apply_profile_version_confirmation_plan(&plan));

        let error = db
            .save_profile_with_confirmation_plan(changed, &plan)
            .expect_err("overlap must be rejected");

        assert!(
            error.to_string().contains("overlap"),
            "unexpected rejection: {error}"
        );
    });
}

#[test]
fn missing_per_year_forms_set_fails_closed_until_user_saves_one() {
    temp_env::with_var("EBIR_TEST_ENV", Some("1"), || {
        let temp_file = NamedTempFile::new().unwrap();
        let db = Database::open(temp_file.path()).expect("Failed to open DB");

        let profile = create_test_profile("000234564000");
        let tin_str = profile.tin.full();

        let _saved_profile = db.save_profile(profile).expect("Failed to save profile");

        db.delete_per_year_forms(&tin_str, 2026).unwrap();

        // Reload profile from DB (without per_year_forms row for 2026)
        let profile_no_forms = db.get_profile(&tin_str).unwrap().unwrap();
        assert!(!db.has_per_year_forms(&tin_str, 2026).unwrap());

        let unresolved = resolve_profile_obligations_for_year(&profile_no_forms, 2026);
        assert!(unresolved.form_codes.is_empty());
        assert!(profile_no_forms.active_form_codes_for_year(2026).is_empty());
        assert!(
            unresolved
                .consistency_report
                .issues
                .iter()
                .any(|issue| issue.code == "FORMS_SET_NOT_CONFIGURED")
        );

        // Now, save a custom forms set to the database for 2026 (precisely containing ONLY ["1701"])
        let forms_set =
            PerYearFormsSet::from_codes(2026, vec!["1701".to_string()], FormSetSource::Manual);
        db.save_per_year_forms(&tin_str, 2026, &forms_set).unwrap();

        // Reload profile from DB
        let profile_with_forms = db.get_profile(&tin_str).unwrap().unwrap();
        assert!(db.has_per_year_forms(&tin_str, 2026).unwrap());
        assert_eq!(
            profile_with_forms
                .per_year_forms
                .get(&2026)
                .unwrap()
                .active_form_codes(),
            vec!["1701".to_string()]
        );

        let resolved = resolve_profile_obligations_for_year(&profile_with_forms, 2026);
        assert_eq!(
            resolved.form_codes,
            vec!["1701".to_string()],
            "Should derive obligations precisely from forms set"
        );
    });
}

#[test]
fn test_custom_form_added_and_removed() {
    temp_env::with_var("EBIR_TEST_ENV", Some("1"), || {
        let temp_file = NamedTempFile::new().unwrap();
        let db = Database::open(temp_file.path()).expect("Failed to open DB");

        let profile = create_test_profile("000234564000");
        let tin_str = profile.tin.full();
        let _saved_profile = db.save_profile(profile).expect("Failed to save profile");

        // 1. Manually add custom form code "CUSTOM_FORM" to the forms set
        let mut set = db.get_per_year_forms(&tin_str, 2026).unwrap();
        let mut custom_entry = FormSetEntry::from_code("CUSTOM_FORM", FormSetSource::Manual);
        custom_entry.frequency = bir_core::forms::registry::FilingFrequency::Quarterly;
        custom_entry.reason = Some("Manual addition of custom form".to_string());
        set.entries.push(custom_entry);

        // Save the updated forms set back
        db.save_per_year_forms(&tin_str, 2026, &set).unwrap();

        // Reload and verify
        let profile_updated = db.get_profile(&tin_str).unwrap().unwrap();
        let resolved_updated = resolve_profile_obligations_for_year(&profile_updated, 2026);
        assert!(
            resolved_updated
                .form_codes
                .contains(&"CUSTOM_FORM".to_string()),
            "Should contain the custom form"
        );

        // 2. Suppress/Deactivate custom form
        let mut set_deactivate = db.get_per_year_forms(&tin_str, 2026).unwrap();
        if let Some(entry) = set_deactivate
            .entries
            .iter_mut()
            .find(|e| e.form_code == "CUSTOM_FORM")
        {
            entry.active = false;
        }
        db.save_per_year_forms(&tin_str, 2026, &set_deactivate)
            .unwrap();

        // Reload and verify
        let profile_deactivated = db.get_profile(&tin_str).unwrap().unwrap();
        let resolved_deactivated = resolve_profile_obligations_for_year(&profile_deactivated, 2026);
        assert!(
            !resolved_deactivated
                .form_codes
                .contains(&"CUSTOM_FORM".to_string()),
            "Should not contain deactivated custom form"
        );
    });
}

#[test]
fn test_cor_confirmation_flow_populates_forms_set() {
    temp_env::with_var("EBIR_TEST_ENV", Some("1"), || {
        let temp_file = NamedTempFile::new().unwrap();
        let db = Database::open(temp_file.path()).expect("Failed to open DB");

        // Create a profile, but manually clear any default confirmed versions so we can simulate draft -> confirm flow.
        let mut profile = create_test_profile("000234564000");
        profile.profile_versions.clear();
        profile.compliance_source_mode = ComplianceSourceMode::TemporalSuggestion;
        let tin_str = profile.tin.full();

        // Add a DRAFT version
        let draft_version = TaxProfileVersion {
            id: "draft-cor-1".to_string(),
            label: "Draft COR Version".to_string(),
            status: TaxProfileVersionStatus::Draft,
            source: TaxProfileVersionSource::OcrCor,
            effective_from: Some(NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
            effective_until: None,
            needs_effective_date_review: false,
            cor: CorRegistrationFacts {
                tin: Some(tin_str.clone()),
                registration_date: Some(NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
                registered_name: "Test Taxpayer".to_string(),
                trade_name: None,
                registered_address: "Manila".to_string(),
                rdo_code: "039".to_string(),
                line_of_business_code: None,
                line_of_business_description: "Services".to_string(),
            },
            registered_tax_types: vec![
                RegisteredTaxType::IncomeTax,
                RegisteredTaxType::PercentageTax,
            ],
            taxpayer_type: TaxpayerType::Individual,
            tax_classification: Some(TaxClassification::SelfEmployed),
            eopt_tier: None,
            is_vat_registered: false,
            is_gpp_partner: false,
            withholds_compensation: false,
            withholds_expanded: false,
            withholds_final: false,
            is_top_withholding_agent: false,
            is_government_withholding_entity: false,
            excise_tax_categories: vec![],
            registration_activity_status: Default::default(),
            evidence: vec![],
            obligation_overrides: vec![ManualObligationOverride {
                form_code: "1701".to_string(),
                action: ManualObligationOverrideAction::Include,
                reason: "Required".to_string(),
                source_reference: None,
            }],
            deadline_overrides: vec![],
        };
        profile.profile_versions.push(draft_version);

        let mut saved_profile = db
            .save_profile(profile)
            .expect("Failed to save initial profile");

        // Verify no forms set exists for 2026 since version is DRAFT
        db.delete_per_year_forms(&tin_str, 2026).unwrap();
        assert!(!db.has_per_year_forms(&tin_str, 2026).unwrap());

        // Confirm through the same reviewed plan boundary used by the app.
        let effective_from = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        let plan = saved_profile
            .profile_version_confirmation_plan("draft-cor-1", effective_from)
            .expect("confirmation plan should be available");
        assert!(saved_profile.apply_profile_version_confirmation_plan(&plan));

        let _final_saved = db
            .save_profile_with_confirmation_plan(saved_profile, &plan)
            .expect("Failed to save confirmed profile");

        // Verify that a forms set has been automatically created and populated in per_year_forms table for 2026!
        assert!(db.has_per_year_forms(&tin_str, 2026).unwrap());
        let generated_set = db.get_per_year_forms(&tin_str, 2026).unwrap();
        assert!(
            !generated_set.entries.is_empty(),
            "Generated forms set should not be empty"
        );

        // The explicit sourced include is authoritative over OCR/tax-type inference.
        let entry = generated_set.entry("1701").unwrap();
        assert!(entry.active);
        assert_eq!(entry.source, FormSetSource::Manual);
    });
}

fn profile_with_open_prior_and_draft(tin: &str) -> TaxpayerProfile {
    let mut profile = create_test_profile(tin);
    profile.profile_versions[0].id = "prior-cor".to_string();
    profile.profile_versions[0].label = "Prior COR".to_string();
    profile.profile_versions[0].source = TaxProfileVersionSource::ManualCor;
    profile.profile_versions[0].effective_from = NaiveDate::from_ymd_opt(2025, 1, 1);
    profile.profile_versions[0].effective_until = None;

    let mut replacement = profile.profile_versions[0].clone();
    replacement.id = "replacement-cor".to_string();
    replacement.label = "Replacement COR".to_string();
    replacement.status = TaxProfileVersionStatus::Draft;
    replacement.source = TaxProfileVersionSource::ManualCor;
    replacement.effective_from = NaiveDate::from_ymd_opt(2026, 7, 1);
    replacement.effective_until = None;
    replacement.needs_effective_date_review = false;
    profile.profile_versions.push(replacement);

    profile
}

fn save_profile_with_open_prior_and_draft(db: &Database, tin: &str) -> TaxpayerProfile {
    let profile = profile_with_open_prior_and_draft(tin);
    let plan = profile
        .profile_version_confirmation_plan(
            "prior-cor",
            NaiveDate::from_ymd_opt(2025, 1, 1).unwrap(),
        )
        .expect("initial reviewed plan should be available");
    db.save_profile_with_confirmation_plan(profile, &plan)
        .expect("initial profile timeline should save")
}

fn save_initial_confirmed_profile(db: &Database, profile: TaxpayerProfile) -> TaxpayerProfile {
    let version = profile
        .profile_versions
        .iter()
        .find(|version| version.status == TaxProfileVersionStatus::Confirmed)
        .expect("fixture should contain one confirmed profile version");
    let effective_from = version
        .effective_from
        .expect("confirmed fixture should have an effective start date");
    let plan = profile
        .profile_version_confirmation_plan(&version.id, effective_from)
        .expect("initial reviewed plan should be available");
    db.save_profile_with_confirmation_plan(profile, &plan)
        .expect("initial reviewed profile should save")
}

fn protected_version_fact_mutations(
    profile: &TaxpayerProfile,
    version_index: usize,
) -> Vec<(&'static str, TaxpayerProfile)> {
    let mut vat_flag = profile.clone();
    vat_flag.profile_versions[version_index].is_vat_registered = true;

    let mut registered_tax_type = profile.clone();
    registered_tax_type.profile_versions[version_index]
        .registered_tax_types
        .push(RegisteredTaxType::ValueAddedTax);

    let mut evidence = profile.clone();
    evidence.profile_versions[version_index]
        .evidence
        .push(CorDocumentRef {
            id: "added-evidence".to_string(),
            file_name: "cor-2303.pdf".to_string(),
            stored_path: "/tmp/cor-2303.pdf".to_string(),
            uploaded_at: None,
            provider: Some("Test".to_string()),
            model: None,
            document_type: Some("COR Form 2303".to_string()),
            extracted_form_codes: vec!["2550Q".to_string()],
            ocr_text: Some("VALUE ADDED TAX".to_string()),
            ocr_confidence: Some(0.99),
            field_bboxes: std::collections::HashMap::new(),
        });

    let mut obligation_override = profile.clone();
    obligation_override.profile_versions[version_index]
        .obligation_overrides
        .push(ManualObligationOverride {
            form_code: "2550Q".to_string(),
            action: ManualObligationOverrideAction::Include,
            reason: "Attempted hidden change".to_string(),
            source_reference: None,
        });

    let mut cor_fact = profile.clone();
    cor_fact.profile_versions[version_index].cor.registered_name =
        "Silently Changed Taxpayer".to_string();

    let mut source = profile.clone();
    source.profile_versions[version_index].source = TaxProfileVersionSource::UserOverride;

    vec![
        ("VAT flag", vat_flag),
        ("registered tax type", registered_tax_type),
        ("evidence and extracted form code", evidence),
        ("obligation override", obligation_override),
        ("COR fact", cor_fact),
        ("source", source),
    ]
}

#[test]
fn initial_exact_migration_backfill_can_save_without_reviewed_plan() {
    temp_env::with_var("EBIR_TEST_ENV", Some("1"), || {
        let temp_file = NamedTempFile::new().unwrap();
        let db = Database::open(temp_file.path()).expect("Failed to open DB");
        let profile = create_test_profile("000234564000");

        let saved = db
            .save_profile(profile)
            .expect("the exact one-time migration backfill should remain compatible");

        assert_eq!(saved.profile_versions.len(), 1);
        assert_eq!(
            saved.profile_versions[0].source,
            TaxProfileVersionSource::MigrationBackfill
        );
        assert_eq!(
            saved.profile_versions[0].status,
            TaxProfileVersionStatus::Confirmed
        );
    });
}

#[test]
fn initial_confirmed_cor_sources_require_an_exact_reviewed_plan() {
    temp_env::with_var("EBIR_TEST_ENV", Some("1"), || {
        for source in [
            TaxProfileVersionSource::ManualCor,
            TaxProfileVersionSource::OcrCor,
            TaxProfileVersionSource::UserOverride,
        ] {
            let temp_file = NamedTempFile::new().unwrap();
            let db = Database::open(temp_file.path()).expect("Failed to open DB");
            let mut profile = create_test_profile("000234564000");
            profile.profile_versions[0].id = "reviewed-cor".to_string();
            profile.profile_versions[0].label = "Reviewed COR".to_string();
            profile.profile_versions[0].source = source.clone();

            let error = db.save_profile(profile.clone()).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("requires an explicit reviewed confirmation plan"),
                "{source:?} unexpectedly saved without review: {error}"
            );

            let effective_from = profile.profile_versions[0].effective_from.unwrap();
            let plan = profile
                .profile_version_confirmation_plan("reviewed-cor", effective_from)
                .expect("reviewed plan should be available");
            db.save_profile_with_confirmation_plan(profile, &plan)
                .expect("the exact first-save reviewed plan should be accepted");
        }
    });
}

#[test]
fn confirmed_profile_version_facts_are_immutable_except_label() {
    temp_env::with_var("EBIR_TEST_ENV", Some("1"), || {
        let temp_file = NamedTempFile::new().unwrap();
        let db = Database::open(temp_file.path()).expect("Failed to open DB");
        let saved = save_profile_with_open_prior_and_draft(&db, "000234564000");

        let mut label_only = saved;
        label_only.profile_versions[0].label = "Renamed confirmed COR".to_string();
        let label_only = db
            .save_profile(label_only)
            .expect("label-only edits remain ordinary metadata changes");

        for (fact, submitted) in protected_version_fact_mutations(&label_only, 0) {
            let error = db.save_profile(submitted).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("Confirmed and archived profile-version facts cannot change"),
                "confirmed {fact} unexpectedly changed: {error}"
            );
        }

        let unchanged = db.get_profile("000234564000").unwrap().unwrap();
        assert_eq!(unchanged.profile_versions[0].label, "Renamed confirmed COR");
        assert!(!unchanged.profile_versions[0].is_vat_registered);
        assert!(unchanged.profile_versions[0].evidence.is_empty());
        assert!(
            unchanged.profile_versions[0]
                .obligation_overrides
                .is_empty()
        );
    });
}

#[test]
fn archived_profile_version_facts_are_immutable_except_label() {
    temp_env::with_var("EBIR_TEST_ENV", Some("1"), || {
        let temp_file = NamedTempFile::new().unwrap();
        let db = Database::open(temp_file.path()).expect("Failed to open DB");
        let mut profile = create_test_profile("000234564000");
        profile.profile_versions[0].status = TaxProfileVersionStatus::Archived;
        profile.profile_versions[0].source = TaxProfileVersionSource::ManualCor;
        let saved = db
            .save_profile(profile)
            .expect("initial archived evidence should save without becoming filing authority");

        let mut label_only = saved;
        label_only.profile_versions[0].label = "Renamed archived COR".to_string();
        let label_only = db
            .save_profile(label_only)
            .expect("archived label-only edits remain ordinary metadata changes");

        for (fact, submitted) in protected_version_fact_mutations(&label_only, 0) {
            let error = db.save_profile(submitted).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("Confirmed and archived profile-version facts cannot change"),
                "archived {fact} unexpectedly changed: {error}"
            );
        }

        let unchanged = db.get_profile("000234564000").unwrap().unwrap();
        assert_eq!(unchanged.profile_versions[0].label, "Renamed archived COR");
        assert!(!unchanged.profile_versions[0].is_vat_registered);
        assert!(unchanged.profile_versions[0].evidence.is_empty());
        assert!(
            unchanged.profile_versions[0]
                .obligation_overrides
                .is_empty()
        );
    });
}

#[test]
fn profile_save_cannot_bypass_reviewed_prior_version_closure() {
    temp_env::with_var("EBIR_TEST_ENV", Some("1"), || {
        let temp_file = NamedTempFile::new().unwrap();
        let db = Database::open(temp_file.path()).expect("Failed to open DB");
        let saved = save_profile_with_open_prior_and_draft(&db, "000234564000");
        let effective_from = NaiveDate::from_ymd_opt(2026, 7, 1).unwrap();
        let plan = saved
            .profile_version_confirmation_plan("replacement-cor", effective_from)
            .unwrap();
        let mut submitted = saved.clone();
        assert!(submitted.apply_profile_version_confirmation_plan(&plan));

        let error = db.save_profile(submitted.clone()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("explicit reviewed confirmation plan is required")
        );

        let unchanged = db.get_profile("000234564000").unwrap().unwrap();
        assert_eq!(unchanged.profile_versions[0].effective_until, None);
        assert_eq!(
            unchanged.profile_versions[1].status,
            TaxProfileVersionStatus::Draft
        );

        let authorized = db
            .save_profile_with_confirmation_plan(submitted, &plan)
            .expect("the exact reviewed plan should authorize the closure");
        assert_eq!(
            authorized.profile_versions[0].effective_until,
            NaiveDate::from_ymd_opt(2026, 6, 30)
        );
        assert_eq!(
            authorized.profile_versions[1].status,
            TaxProfileVersionStatus::Confirmed
        );
    });
}

#[test]
fn profile_save_rejects_two_step_manual_close_then_planless_confirmation_bypass() {
    temp_env::with_var("EBIR_TEST_ENV", Some("1"), || {
        let close_temp_file = NamedTempFile::new().unwrap();
        let close_db = Database::open(close_temp_file.path()).expect("Failed to open DB");
        let saved = save_profile_with_open_prior_and_draft(&close_db, "000234564000");

        let mut manually_closed = saved;
        manually_closed.profile_versions[0].effective_until = NaiveDate::from_ymd_opt(2026, 6, 30);
        let error = close_db.save_profile(manually_closed).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Confirmed and archived profile-version facts cannot change")
        );
        let unchanged = close_db.get_profile("000234564000").unwrap().unwrap();
        assert_eq!(unchanged.profile_versions[0].effective_until, None);

        // A confirmation with no predecessor closure is still a reviewed
        // transition. This prevents the second half of a split save from
        // becoming planless merely because no open predecessor remains.
        let confirm_temp_file = NamedTempFile::new().unwrap();
        let confirm_db = Database::open(confirm_temp_file.path()).expect("Failed to open DB");
        let mut draft_only = create_test_profile("000234564000");
        let mut replacement = draft_only.profile_versions.remove(0);
        replacement.id = "replacement-cor".to_string();
        replacement.label = "Replacement COR".to_string();
        replacement.status = TaxProfileVersionStatus::Draft;
        replacement.source = TaxProfileVersionSource::ManualCor;
        replacement.effective_from = NaiveDate::from_ymd_opt(2026, 7, 1);
        replacement.effective_until = None;
        draft_only.profile_versions.push(replacement);
        let saved = confirm_db
            .save_profile(draft_only)
            .expect("an unconfirmed draft should save initially");
        let effective_from = NaiveDate::from_ymd_opt(2026, 7, 1).unwrap();
        let plan = saved
            .profile_version_confirmation_plan("replacement-cor", effective_from)
            .unwrap();
        assert!(plan.auto_close_consequences.is_empty());
        let mut submitted = saved;
        assert!(submitted.apply_profile_version_confirmation_plan(&plan));

        let error = confirm_db.save_profile(submitted.clone()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("explicit reviewed confirmation plan is required")
        );
        let unchanged = confirm_db.get_profile("000234564000").unwrap().unwrap();
        assert_eq!(
            unchanged.profile_versions[0].status,
            TaxProfileVersionStatus::Draft
        );

        confirm_db
            .save_profile_with_confirmation_plan(submitted, &plan)
            .expect("the current reviewed plan should authorize confirmation");
    });
}

#[test]
fn ordinary_profile_save_rejects_direct_confirmed_timeline_edits() {
    temp_env::with_var("EBIR_TEST_ENV", Some("1"), || {
        let temp_file = NamedTempFile::new().unwrap();
        let db = Database::open(temp_file.path()).expect("Failed to open DB");
        let saved = save_profile_with_open_prior_and_draft(&db, "000234564000");

        let mut changed_start = saved.clone();
        changed_start.profile_versions[0].effective_from = NaiveDate::from_ymd_opt(2025, 2, 1);
        let mut changed_end = saved.clone();
        changed_end.profile_versions[0].effective_until = NaiveDate::from_ymd_opt(2026, 6, 30);
        let mut changed_status = saved.clone();
        changed_status.profile_versions[0].status = TaxProfileVersionStatus::Archived;
        let mut changed_review_state = saved.clone();
        changed_review_state.profile_versions[0].needs_effective_date_review = true;

        for (field, submitted) in [
            ("effective_from", changed_start),
            ("effective_until", changed_end),
            ("status", changed_status),
            ("needs_effective_date_review", changed_review_state),
        ] {
            let error = db.save_profile(submitted).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("Confirmed and archived profile-version facts cannot change"),
                "{field} unexpectedly bypassed the persistence boundary: {error}"
            );
        }

        let unchanged = db.get_profile("000234564000").unwrap().unwrap();
        assert_eq!(
            unchanged.profile_versions[0].effective_from,
            NaiveDate::from_ymd_opt(2025, 1, 1)
        );
        assert_eq!(unchanged.profile_versions[0].effective_until, None);
        assert_eq!(
            unchanged.profile_versions[0].status,
            TaxProfileVersionStatus::Confirmed
        );
        assert!(!unchanged.profile_versions[0].needs_effective_date_review);
    });
}

#[test]
fn reviewed_confirmation_plan_cannot_authorize_extra_timeline_edits() {
    temp_env::with_var("EBIR_TEST_ENV", Some("1"), || {
        let temp_file = NamedTempFile::new().unwrap();
        let db = Database::open(temp_file.path()).expect("Failed to open DB");
        let saved = save_profile_with_open_prior_and_draft(&db, "000234564000");
        let effective_from = NaiveDate::from_ymd_opt(2026, 7, 1).unwrap();
        let plan = saved
            .profile_version_confirmation_plan("replacement-cor", effective_from)
            .unwrap();
        let mut submitted = saved;
        assert!(submitted.apply_profile_version_confirmation_plan(&plan));
        submitted.profile_versions[0].effective_from = NaiveDate::from_ymd_opt(2025, 2, 1);

        let error = db
            .save_profile_with_confirmation_plan(submitted, &plan)
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Confirmed and archived profile-version facts cannot change")
        );

        let unchanged = db.get_profile("000234564000").unwrap().unwrap();
        assert_eq!(
            unchanged.profile_versions[0].effective_from,
            NaiveDate::from_ymd_opt(2025, 1, 1)
        );
        assert_eq!(unchanged.profile_versions[0].effective_until, None);
        assert_eq!(
            unchanged.profile_versions[1].status,
            TaxProfileVersionStatus::Draft
        );
    });
}

#[test]
fn reviewed_confirmation_plan_cannot_resurrect_an_archived_version() {
    temp_env::with_var("EBIR_TEST_ENV", Some("1"), || {
        let temp_file = NamedTempFile::new().unwrap();
        let db = Database::open(temp_file.path()).expect("Failed to open DB");
        let mut profile = profile_with_open_prior_and_draft("000234564000");
        profile.profile_versions[0].status = TaxProfileVersionStatus::Archived;
        profile.profile_versions[0].effective_until = NaiveDate::from_ymd_opt(2025, 12, 31);
        let saved = db
            .save_profile(profile)
            .expect("initial archived evidence should save");
        let effective_from = NaiveDate::from_ymd_opt(2025, 1, 1).unwrap();
        let plan = saved
            .profile_version_confirmation_plan("prior-cor", effective_from)
            .expect("domain planner can describe the attempted transition");
        let mut submitted = saved;
        assert!(submitted.apply_profile_version_confirmation_plan(&plan));

        let error = db
            .save_profile_with_confirmation_plan(submitted, &plan)
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Archived profile versions cannot be confirmed directly")
        );

        let unchanged = db.get_profile("000234564000").unwrap().unwrap();
        assert_eq!(
            unchanged.profile_versions[0].status,
            TaxProfileVersionStatus::Archived
        );
        assert_eq!(
            unchanged.profile_versions[0].effective_until,
            NaiveDate::from_ymd_opt(2025, 12, 31)
        );
    });
}

#[test]
fn profile_save_rejects_stale_reviewed_confirmation_plan() {
    temp_env::with_var("EBIR_TEST_ENV", Some("1"), || {
        let temp_file = NamedTempFile::new().unwrap();
        let db = Database::open(temp_file.path()).expect("Failed to open DB");
        let original = save_profile_with_open_prior_and_draft(&db, "000234564000");
        let effective_from = NaiveDate::from_ymd_opt(2026, 7, 1).unwrap();
        let stale_plan = original
            .profile_version_confirmation_plan("replacement-cor", effective_from)
            .unwrap();

        let mut stored_edit = original.clone();
        stored_edit.profile_versions[0].label = "Prior COR edited after review".to_string();
        db.save_profile(stored_edit)
            .expect("ordinary non-timeline profile edit should save");

        let mut stale_submission = original;
        assert!(stale_submission.apply_profile_version_confirmation_plan(&stale_plan));
        let error = db
            .save_profile_with_confirmation_plan(stale_submission, &stale_plan)
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("timeline changed after confirmation was reviewed")
        );

        let unchanged = db.get_profile("000234564000").unwrap().unwrap();
        assert_eq!(
            unchanged.profile_versions[0].label,
            "Prior COR edited after review"
        );
        assert_eq!(unchanged.profile_versions[0].effective_until, None);
        assert_eq!(
            unchanged.profile_versions[1].status,
            TaxProfileVersionStatus::Draft
        );
    });
}

#[test]
fn test_obligation_filtering_individual_vs_corporate_and_vat() {
    temp_env::with_var("EBIR_TEST_ENV", Some("1"), || {
        let temp_file = NamedTempFile::new().unwrap();
        let db = Database::open(temp_file.path()).expect("Failed to open DB");

        // 1. Create Individual profile
        let mut individual = create_test_profile("000234564000");
        individual.taxpayer_type = TaxpayerType::Individual;
        individual.tax_classification = Some(TaxClassification::SelfEmployed);
        individual.compliance_source_mode = ComplianceSourceMode::CorVersioned;
        individual.profile_versions[0] = TaxProfileVersion {
            id: "v-indiv".to_string(),
            label: "Indiv Version".to_string(),
            status: TaxProfileVersionStatus::Confirmed,
            source: TaxProfileVersionSource::ManualCor,
            effective_from: Some(NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
            effective_until: None,
            needs_effective_date_review: false,
            cor: CorRegistrationFacts {
                tin: Some("000234564000".to_string()),
                registration_date: Some(NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
                registered_name: "Test Indiv".to_string(),
                trade_name: None,
                registered_address: "QC".to_string(),
                rdo_code: "039".to_string(),
                line_of_business_code: None,
                line_of_business_description: "Services".to_string(),
            },
            registered_tax_types: vec![
                RegisteredTaxType::IncomeTax,
                RegisteredTaxType::PercentageTax,
            ],
            taxpayer_type: TaxpayerType::Individual,
            tax_classification: Some(TaxClassification::SelfEmployed),
            eopt_tier: None,
            is_vat_registered: false,
            is_gpp_partner: false,
            withholds_compensation: false,
            withholds_expanded: false,
            withholds_final: false,
            is_top_withholding_agent: false,
            is_government_withholding_entity: false,
            excise_tax_categories: vec![],
            registration_activity_status: Default::default(),
            evidence: vec![],
            obligation_overrides: vec![],
            deadline_overrides: vec![],
        };

        let saved_indiv = save_initial_confirmed_profile(&db, individual);
        let resolved_indiv = resolve_profile_obligations_for_year(&saved_indiv, 2026);

        // Individual should have 1701/1701Q but NOT 1702/1702Q
        assert!(resolved_indiv.form_codes.contains(&"1701".to_string()));
        assert!(resolved_indiv.form_codes.contains(&"1701Q".to_string()));
        assert!(!resolved_indiv.form_codes.contains(&"1702RT".to_string()));
        assert!(!resolved_indiv.form_codes.contains(&"1702Q".to_string()));

        // 2. Create Corporate profile
        let mut corporate = create_test_profile("987654321000");
        corporate.taxpayer_type = TaxpayerType::Corporation;
        corporate.tax_classification = Some(TaxClassification::Corporation);
        corporate.compliance_source_mode = ComplianceSourceMode::CorVersioned;
        corporate.profile_versions[0] = TaxProfileVersion {
            id: "v-corp".to_string(),
            label: "Corp Version".to_string(),
            status: TaxProfileVersionStatus::Confirmed,
            source: TaxProfileVersionSource::ManualCor,
            effective_from: Some(NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
            effective_until: None,
            needs_effective_date_review: false,
            cor: CorRegistrationFacts {
                tin: Some("987654321000".to_string()),
                registration_date: Some(NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
                registered_name: "Test Corp".to_string(),
                trade_name: None,
                registered_address: "Manila".to_string(),
                rdo_code: "039".to_string(),
                line_of_business_code: None,
                line_of_business_description: "Services".to_string(),
            },
            registered_tax_types: vec![RegisteredTaxType::IncomeTax],
            taxpayer_type: TaxpayerType::Corporation,
            tax_classification: Some(TaxClassification::Corporation),
            eopt_tier: None,
            is_vat_registered: false,
            is_gpp_partner: false,
            withholds_compensation: false,
            withholds_expanded: false,
            withholds_final: false,
            is_top_withholding_agent: false,
            is_government_withholding_entity: false,
            excise_tax_categories: vec![],
            registration_activity_status: Default::default(),
            evidence: vec![],
            obligation_overrides: vec![],
            deadline_overrides: vec![],
        };

        let saved_corp = save_initial_confirmed_profile(&db, corporate);
        let resolved_corp = resolve_profile_obligations_for_year(&saved_corp, 2026);

        // Corp should have 1702RT/1702Q but NOT 1701/1701Q
        assert!(!resolved_corp.form_codes.contains(&"1701".to_string()));
        assert!(!resolved_corp.form_codes.contains(&"1701Q".to_string()));
        assert!(resolved_corp.form_codes.contains(&"1702RT".to_string()));
        assert!(resolved_corp.form_codes.contains(&"1702Q".to_string()));

        // 3. Create VAT-registered Corporate profile
        let mut vat_corp = create_test_profile("111222333000");
        vat_corp.taxpayer_type = TaxpayerType::Corporation;
        vat_corp.tax_classification = Some(TaxClassification::Corporation);
        vat_corp.is_vat_registered = true;
        vat_corp.compliance_source_mode = ComplianceSourceMode::CorVersioned;
        vat_corp.profile_versions[0] = TaxProfileVersion {
            id: "v-vat-corp".to_string(),
            label: "VAT Corp Version".to_string(),
            status: TaxProfileVersionStatus::Confirmed,
            source: TaxProfileVersionSource::ManualCor,
            effective_from: Some(NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
            effective_until: None,
            needs_effective_date_review: false,
            cor: CorRegistrationFacts {
                tin: Some("111222333000".to_string()),
                registration_date: Some(NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
                registered_name: "Test VAT Corp".to_string(),
                trade_name: None,
                registered_address: "Manila".to_string(),
                rdo_code: "039".to_string(),
                line_of_business_code: None,
                line_of_business_description: "Services".to_string(),
            },
            registered_tax_types: vec![
                RegisteredTaxType::IncomeTax,
                RegisteredTaxType::ValueAddedTax,
            ],
            taxpayer_type: TaxpayerType::Corporation,
            tax_classification: Some(TaxClassification::Corporation),
            eopt_tier: None,
            is_vat_registered: true,
            is_gpp_partner: false,
            withholds_compensation: false,
            withholds_expanded: false,
            withholds_final: false,
            is_top_withholding_agent: false,
            is_government_withholding_entity: false,
            excise_tax_categories: vec![],
            registration_activity_status: Default::default(),
            evidence: vec![],
            obligation_overrides: vec![],
            deadline_overrides: vec![],
        };

        let saved_vat_corp = save_initial_confirmed_profile(&db, vat_corp);
        let resolved_vat_corp = resolve_profile_obligations_for_year(&saved_vat_corp, 2026);

        // VAT Corp should have quarterly VAT but not percentage tax.
        assert!(resolved_vat_corp.form_codes.contains(&"2550Q".to_string()));
        assert!(!resolved_vat_corp.form_codes.contains(&"2551Q".to_string()));
    });
}
