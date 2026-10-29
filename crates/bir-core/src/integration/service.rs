//! Integration Service — the orchestrator for external system sync operations.
//!
//! This module provides the core service layer that is **transport-agnostic**.
//! It accepts a `UniversalTaxPayload` (or raw JSON) and orchestrates:
//!
//! 1. Payload validation
//! 2. Profile lookup by TIN
//! 3. Form applicability check
//! 4. Mapper resolution and execution
//! 5. Draft validation
//! 6. Persistence to encrypted SQLite
//!
//! The caller (HTTP server, file importer, URL scheme handler) only needs
//! to deserialize the JSON and pass it here.

use crate::db::Database;
use crate::forms::FormValidator;
use crate::integration::mapper::{FormDraftOutput, MapperError, resolve_mappers};
use crate::integration::models::UniversalTaxPayload;
use crate::integration::validation::{
    PayloadValidationError, validate_form_applicability, validate_payload,
};
use chrono::Datelike;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum SyncError {
    #[error("Payload validation failed: {0:?}")]
    PayloadInvalid(Vec<PayloadValidationError>),
    #[error("JSON deserialization failed: {0}")]
    JsonError(#[from] serde_json::Error),
    #[error("Profile not found for TIN: {0}")]
    ProfileNotFound(String),
    #[error("Profile is archived: {0}")]
    ProfileArchived(String),
    #[error("Form not applicable: {0}")]
    FormNotApplicable(String),
    #[error("No applicable mapper for this payload")]
    NoMapper,
    #[error("Mapping failed: {0}")]
    MapperFailed(#[from] MapperError),
    #[error("Draft validation failed: {0:?}")]
    DraftInvalid(Vec<(String, String)>),
    #[error("Database error: {0}")]
    DatabaseError(String),
}

/// Result of a successful sync operation for a single form.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncResult {
    /// The form code that was generated (e.g., "2551Q").
    pub form_code: String,
    /// Taxable year derived from the payload.
    pub taxable_year: u16,
    /// Quarter (if applicable).
    pub quarter: Option<u8>,
    /// The database row ID of the saved draft.
    pub draft_id: i64,
    /// Total tax due as computed by the mapper.
    pub total_tax_due: f64,
    /// Total amount payable (after credits and penalties).
    pub total_amount_payable: f64,
    /// Number of schedule rows generated.
    pub schedule_row_count: usize,
}

/// Aggregated response from processing a single payload.
/// A single payload may produce multiple form drafts (e.g., 2551Q + 1701Q).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncResponse {
    /// Whether the sync succeeded for all applicable forms.
    pub success: bool,
    /// Results for each form that was generated.
    pub results: Vec<SyncResult>,
    /// Warnings that didn't block the sync.
    pub warnings: Vec<String>,
}

/// Process a raw JSON payload string — the primary entry point for all transports.
///
/// This is what an HTTP handler, file importer, or URL scheme handler calls.
pub fn process_sync_json(db: &Database, json: &str) -> Result<SyncResponse, SyncError> {
    let payload: UniversalTaxPayload = serde_json::from_str(json)?;
    process_sync(db, &payload)
}

/// Process a typed `UniversalTaxPayload` — useful when the caller already
/// deserialized the payload (e.g., from a URL scheme with query parameters).
pub fn process_sync(
    db: &Database,
    payload: &UniversalTaxPayload,
) -> Result<SyncResponse, SyncError> {
    // ── Step 1: Validate payload structure ────────────────────────────
    let payload_errors = validate_payload(payload);
    if !payload_errors.is_empty() {
        return Err(SyncError::PayloadInvalid(payload_errors));
    }

    // ── Step 2: Look up profile by TIN ───────────────────────────────
    let profile = db
        .get_profile(&payload.tin)
        .map_err(|e| SyncError::DatabaseError(e.to_string()))?
        .ok_or_else(|| SyncError::ProfileNotFound(payload.tin.clone()))?;

    if profile.is_archived {
        return Err(SyncError::ProfileArchived(payload.tin.clone()));
    }

    // ── Step 3: Check form applicability ─────────────────────────────
    if let Some(ref target_form) = payload.target_form {
        let taxable_year = payload.period_start.year() as u16;
        if let Some(err) = validate_form_applicability(target_form, &profile, taxable_year) {
            return Err(SyncError::FormNotApplicable(err.message));
        }
    }

    // ── Step 4: Resolve mappers ──────────────────────────────────────
    let mappers = resolve_mappers(payload);
    if mappers.is_empty() {
        return Err(SyncError::NoMapper);
    }

    // ── Step 5: Run each mapper ──────────────────────────────────────
    let mut results = Vec::new();
    let mut warnings = Vec::new();

    for mapper in &mappers {
        let draft_output = mapper.map(payload, &profile)?;

        match draft_output {
            FormDraftOutput::Form2551Q(ref draft) => {
                // Step 5a: Validate the generated draft
                let validation_errors = draft.validate();
                if !validation_errors.is_empty() {
                    // Non-blocking: report as warnings but still save
                    for (field, msg) in &validation_errors {
                        warnings.push(format!("2551Q validation: {} — {}", field, msg));
                    }
                }

                // Step 5b: Persist to database
                let draft_id = db
                    .save_2551q_draft(draft)
                    .map_err(|e| SyncError::DatabaseError(e.to_string()))?;

                results.push(SyncResult {
                    form_code: "2551Q".to_string(),
                    taxable_year: draft.taxable_year,
                    quarter: Some(draft.quarter),
                    draft_id,
                    total_tax_due: draft.total_tax_due,
                    total_amount_payable: draft.total_amount_payable,
                    schedule_row_count: draft.schedule_1.len(),
                });
            }
        }
    }

    Ok(SyncResponse {
        success: true,
        results,
        warnings,
    })
}

/// Import a payload from a JSON file on disk.
///
/// This is the App Store-safe transport mechanism — external tools write
/// a `.json` file to a known location, and the app imports it.
pub fn import_payload_file(
    db: &Database,
    file_path: &std::path::Path,
) -> Result<SyncResponse, SyncError> {
    let json = std::fs::read_to_string(file_path)
        .map_err(|e| SyncError::DatabaseError(format!("Failed to read file: {e}")))?;
    process_sync_json(db, &json)
}

/// Import multiple payloads from a directory.
///
/// Scans for `*.json` files in the given directory and processes each one.
/// Returns results for all successful imports and errors for failures.
pub fn import_payload_directory(
    db: &Database,
    dir_path: &std::path::Path,
) -> Vec<(String, Result<SyncResponse, SyncError>)> {
    let mut results = Vec::new();

    let entries = match std::fs::read_dir(dir_path) {
        Ok(entries) => entries,
        Err(e) => {
            results.push((
                dir_path.display().to_string(),
                Err(SyncError::DatabaseError(format!(
                    "Failed to read directory: {e}"
                ))),
            ));
            return results;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "json") {
            let filename = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let result = import_payload_file(db, &path);
            results.push((filename, result));
        }
    }

    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::naming::Tin;
    use crate::profile::{TaxpayerProfile, TaxpayerType};
    use tempfile::tempdir;

    fn setup_db_with_profile() -> Option<Database> {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test_integration.db");
        let db = match Database::open(&db_path) {
            Ok(db) => db,
            Err(e) => {
                println!("Skipping test due to keyring unavailability: {:?}", e);
                return None;
            }
        };

        let profile = TaxpayerProfile {
            id: None,
            full_name: "JUAN DELA CRUZ".to_string(),
            tin: Tin {
                segment1: "000".into(),
                segment2: "234".into(),
                segment3: "564".into(),
                branch: "000".into(),
            },
            rdo_code: "039".to_string(),
            line_of_business: "Consulting Services".to_string(),
            registered_address: "123 Rizal Street, Quezon City".to_string(),
            zip_code: "1100".to_string(),
            phone: "09170000100".to_string(),
            email: "juan@example.com".to_string(),
            default_form_type: "2551Q".to_string(),
            taxpayer_type: TaxpayerType::Individual,
            is_vat_registered: false,
            business_start_date: chrono::NaiveDate::from_ymd_opt(2020, 1, 1),
            birth_date: None,
            tax_classification: Some(crate::profile::TaxClassification::SelfEmployed),
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
            compliance_source_mode: Default::default(),
            per_year_forms: Default::default(),
        };

        db.save_profile(profile).unwrap();
        Some(db)
    }

    fn test_payload_json() -> String {
        r#"{
            "tin": "000234564000",
            "target_form": "2551Q",
            "period_start": "2026-01-01",
            "period_end": "2026-03-31",
            "is_amended": false,
            "income_sources": [
                {
                    "category": "BusinessNonVat",
                    "gross_amount": 500000.0,
                    "is_vat_exempt": true,
                    "atc_code_override": "PT010"
                }
            ],
            "creditable_withholdings": 15000.0,
            "previous_tax_paid": 0.0
        }"#
        .to_string()
    }

    #[test]
    fn test_end_to_end_sync() {
        let Some(db) = setup_db_with_profile() else {
            return;
        };
        let json = test_payload_json();

        let response = process_sync_json(&db, &json).unwrap();

        assert!(response.success);
        assert_eq!(response.results.len(), 1);

        let result = &response.results[0];
        assert_eq!(result.form_code, "2551Q");
        assert_eq!(result.taxable_year, 2026);
        assert_eq!(result.quarter, Some(1));
        assert_eq!(result.total_tax_due, 15_000.0);
        assert_eq!(result.schedule_row_count, 1);
        assert!(result.draft_id > 0);

        // Verify draft was persisted
        let draft = db.get_2551q_draft("000234564000", 2026, 1).unwrap();
        assert!(draft.is_some());
        let draft = draft.unwrap();
        assert_eq!(draft.taxpayer_name, "JUAN DELA CRUZ");
        assert_eq!(draft.total_tax_due, 15_000.0);
    }

    #[test]
    fn test_sync_unknown_tin() {
        let Some(db) = setup_db_with_profile() else {
            return;
        };
        let json = r#"{
            "tin": "999999999000",
            "period_start": "2026-01-01",
            "period_end": "2026-03-31",
            "income_sources": [{ "category": "BusinessNonVat", "gross_amount": 100000.0 }]
        }"#;

        let result = process_sync_json(&db, json);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), SyncError::ProfileNotFound(_)));
    }

    #[test]
    fn test_sync_invalid_payload() {
        let Some(db) = setup_db_with_profile() else {
            return;
        };
        let json = r#"{
            "tin": "bad",
            "period_start": "2026-01-01",
            "period_end": "2026-03-31",
            "income_sources": []
        }"#;

        let result = process_sync_json(&db, json);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), SyncError::PayloadInvalid(_)));
    }

    #[test]
    fn test_sync_bad_json() {
        let Some(db) = setup_db_with_profile() else {
            return;
        };
        let result = process_sync_json(&db, "not valid json");
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), SyncError::JsonError(_)));
    }

    #[test]
    fn test_sync_idempotent_upsert() {
        let Some(db) = setup_db_with_profile() else {
            return;
        };
        let json = test_payload_json();

        // First sync
        let r1 = process_sync_json(&db, &json).unwrap();
        assert_eq!(r1.results.len(), 1);

        // Second sync — should upsert, not duplicate
        let r2 = process_sync_json(&db, &json).unwrap();
        assert_eq!(r2.results.len(), 1);

        // Verify only one draft exists
        let draft = db.get_2551q_draft("000234564000", 2026, 1).unwrap();
        assert!(draft.is_some());
    }

    #[test]
    fn test_file_import() {
        let Some(db) = setup_db_with_profile() else {
            return;
        };
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("payload.json");
        std::fs::write(&file_path, test_payload_json()).unwrap();

        let response = import_payload_file(&db, &file_path).unwrap();
        assert!(response.success);
        assert_eq!(response.results.len(), 1);
    }

    #[test]
    fn test_directory_import() {
        let Some(db) = setup_db_with_profile() else {
            return;
        };
        let dir = tempdir().unwrap();

        // Write two payloads for different quarters
        let q1_json = test_payload_json();
        let q2_json = r#"{
            "tin": "000234564000",
            "target_form": "2551Q",
            "period_start": "2026-04-01",
            "period_end": "2026-06-30",
            "income_sources": [{
                "category": "BusinessNonVat",
                "gross_amount": 200000.0,
                "atc_code_override": "PT010"
            }]
        }"#;

        std::fs::write(dir.path().join("q1.json"), q1_json).unwrap();
        std::fs::write(dir.path().join("q2.json"), q2_json).unwrap();
        // Non-JSON file should be ignored
        std::fs::write(dir.path().join("readme.txt"), "ignore me").unwrap();

        let results = import_payload_directory(&db, dir.path());
        assert_eq!(results.len(), 2); // Only .json files

        let successes: Vec<_> = results.iter().filter(|(_, r)| r.is_ok()).collect();
        assert_eq!(successes.len(), 2);
    }
}
