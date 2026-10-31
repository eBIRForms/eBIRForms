use bir_core::db::{Database, PostCommitRefreshStatus};
use bir_core::profile::TaxpayerProfile;
use tempfile::NamedTempFile;

fn test_profile() -> TaxpayerProfile {
    serde_json::from_value(serde_json::json!({
        "id": null,
        "full_name": "Post Commit Refresh Test",
        "tin": {
            "segment1": "741",
            "segment2": "852",
            "segment3": "963",
            "branch": "000"
        },
        "rdo_code": "018",
        "line_of_business": "Testing Services",
        "registered_address": "Olongapo City",
        "zip_code": "2200",
        "phone": "09123456789",
        "email": "refresh@example.com",
        "default_form_type": "2551Qv2018",
        "taxpayer_type": "Individual",
        "business_start_date": "2020-01-01"
    }))
    .expect("minimal taxpayer profile fixture should deserialize")
}

#[test]
fn profile_save_reports_recorded_post_commit_refresh_request() {
    temp_env::with_var("BUWIZ_TEST_ENV", Some("1"), || {
        let file = NamedTempFile::new().expect("temporary database should open");
        let db = Database::open(file.path()).expect("database should initialize");

        let outcome = db
            .save_profile_with_post_commit_status(test_profile())
            .expect("profile transaction should commit");

        assert!(matches!(
            outcome.refresh_status(),
            PostCommitRefreshStatus::Deferred { warning }
                if warning.contains("committed") && warning.contains("not running yet")
        ));
        assert!(outcome.committed().id.is_some());
    });
}
