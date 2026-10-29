use bir_core::db::Database;
use bir_core::forms::form_2551q::Item13Election;
use bir_core::forms::{FilingStatus, Form2551QDraft};
use bir_core::naming::Tin;
use bir_core::profile::TaxpayerProfile;
use tempfile::NamedTempFile;

#[test]
fn test_full_form_lifecycle() {
    temp_env::with_var("EBIR_TEST_ENV", Some("1"), || {
        // 1. Initialize temporary encrypted database
        let temp_file = NamedTempFile::new().unwrap();
        let db = Database::open(temp_file.path()).expect("Failed to open temp DB");

        // 2. Create and save profile
        let mut profile = TaxpayerProfile {
            id: None,
            full_name: "Integration Test User".to_string(),
            tin: Tin {
                segment1: "123".into(),
                segment2: "456".into(),
                segment3: "789".into(),
                branch: "000".into(),
            },
            rdo_code: "039".to_string(),
            line_of_business: "Test".to_string(),
            registered_address: "Manila".to_string(),
            zip_code: "1000".to_string(),
            phone: "09000000000".to_string(),
            email: "test@example.com".to_string(),
            default_form_type: "2551Q".to_string(),
            taxpayer_type: bir_core::profile::TaxpayerType::Individual,
            is_vat_registered: false,
            business_start_date: chrono::NaiveDate::from_ymd_opt(2020, 1, 1),
            birth_date: None,
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
            is_archived: false,
            profile_pin_hash: None,
            totp_secret: None,
            email_tracking_enabled: false,
            email_auth_method: bir_core::profile::EmailAuthMethod::AppPassword,
            imap_email: None,
            imap_host: None,
            test_notification_enabled: false,
            imap_app_password: None,
            oauth_access_token: None,
            oauth_refresh_token: None,
            profile_versions: vec![],
            compliance_source_mode: Default::default(),
            per_year_forms: Default::default(),
        };
        profile.ensure_profile_version_ledger();
        let saved_profile = db.save_profile(profile).expect("Failed to save profile");

        // 3. Initialize Draft
        let mut draft = Form2551QDraft::new_from_effective_profile(&saved_profile, 2026, 1);
        assert_eq!(draft.status, FilingStatus::Draft);
        assert_eq!(draft.rdo_code, "039");

        // 4. User edits schedule 1
        draft.schedule_1[0].taxable_amount = 100_000.0;

        // 5. Recompute
        draft.recompute(None);

        // Assuming PT010 rate is 3%
        assert_eq!(draft.total_tax_due, 3_000.0);
        assert_eq!(draft.tax_payable, 3_000.0);

        // 6. Save Draft
        db.save_2551q_draft(&draft).expect("Failed to save draft");

        // 7. Load Draft and Verify
        let loaded_draft = db
            .get_2551q_draft(&saved_profile.tin.full(), 2026, 1)
            .expect("DB error")
            .expect("Draft not found");

        assert_eq!(loaded_draft.status, FilingStatus::Draft);
        assert_eq!(loaded_draft.total_tax_due, 3_000.0);

        // 8. Queue for submission
        let mut final_draft = loaded_draft;
        final_draft.item_13_election = Item13Election::Graduated;
        final_draft
            .transition_to_queued()
            .expect("Loaded valid draft should transition to Queued");
        db.save_queued_2551q_draft_and_election(&final_draft)
            .expect("Failed to save queued draft");

        // 9. Verify Queue State
        let queued = db
            .get_2551q_draft(&saved_profile.tin.full(), 2026, 1)
            .expect("DB error")
            .expect("Draft not found");

        assert_eq!(queued.status, FilingStatus::Queued);
    });
}
