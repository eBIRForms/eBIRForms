use crate::db::{Database, DbError, Submission};
use crate::profile::TaxpayerProfile;
use std::fs;
use std::io::Read;
use std::path::Path;
use tracing::info;
use zip::ZipArchive;

/// Parse a semver string like "0.0.1" into (major, minor, patch).
/// Returns (0, 0, 0) for unparseable strings.
fn parse_semver(s: &str) -> (u32, u32, u32) {
    let parts: Vec<&str> = s.trim().split('.').collect();
    let major = parts.first().and_then(|p| p.parse().ok()).unwrap_or(0);
    let minor = parts.get(1).and_then(|p| p.parse().ok()).unwrap_or(0);
    let patch = parts.get(2).and_then(|p| p.parse().ok()).unwrap_or(0);
    (major, minor, patch)
}

/// Apply sequential JSON-level migrations to a profile Value before deserialization.
///
/// Each migration step transforms the JSON structure to match the current schema.
/// The `from_version` is the semver tuple from the archive's manifest.json.
///
/// When you bump the Cargo package version AND change the export schema, add a new
/// `if from_version < (M, N, P)` block here with the corresponding field transformations.
fn migrate_profile_json(
    _value: &mut serde_json::Value,
    from_version: (u32, u32, u32),
) -> Result<(), DbError> {
    // Example for future use:
    // if from_version < (0, 2, 0) {
    //     // < 0.2.0: renamed `rdo_code` → `rdo`
    //     if let Some(old) = value.get("rdo_code").cloned() {
    //         value["rdo"] = old;
    //         value.as_object_mut().unwrap().remove("rdo_code");
    //     }
    // }
    if from_version < (0, 0, 1) {
        // Pre-0.0.1 (legacy unversioned): no structural changes needed.
        // All newer fields have #[serde(default)].
    }
    Ok(())
}

/// Apply sequential JSON-level migrations to a submission Value before deserialization.
fn migrate_submission_json(
    _value: &mut serde_json::Value,
    from_version: (u32, u32, u32),
) -> Result<(), DbError> {
    if from_version < (0, 0, 1) {
        // Pre-0.0.1: no changes needed
    }
    Ok(())
}

pub fn import_profile_data(db: &Database, import_file: &Path) -> Result<(), DbError> {
    let file = fs::File::open(import_file)?;
    let mut archive = ZipArchive::new(file).map_err(|e| DbError::Other(e.to_string()))?;

    let mut base_dirs = Vec::new();
    for i in 0..archive.len() {
        if let Ok(file) = archive.by_index(i) {
            let name = file.name();
            if name.ends_with("profile.json") {
                let Some(base_dir) = name.strip_suffix("profile.json") else {
                    continue;
                };
                base_dirs.push(base_dir.to_string());
            }
        }
    }

    if base_dirs.is_empty() {
        return Err(DbError::Other(
            "No profile.json found in archive".to_string(),
        ));
    }

    // The whole archive writes under one transaction: a failure partway —
    // a later profile's malformed JSON, a full disk — must not leave some
    // profiles imported and some not. `save_profile` rides this transaction
    // (see its nested-transaction note); COMMIT lands all of it, ROLLBACK
    // none of it.
    db.conn.execute_batch("BEGIN IMMEDIATE")?;
    let result = (|| -> Result<(), DbError> {
        for base_dir in base_dirs {
            // Helper to read file from archive
            let mut read_from_zip = |name: &str| -> Result<String, DbError> {
                let full_name = format!("{}{}", base_dir, name);
                let mut file = archive
                    .by_name(&full_name)
                    .map_err(|e| DbError::Other(e.to_string()))?;
                let mut content = String::new();
                file.read_to_string(&mut content)?;
                Ok(content)
            };

            // 0. Read manifest.json for export version (defaults to (0,0,0) for legacy archives)
            let export_version = match read_from_zip("manifest.json") {
                Ok(manifest_str) => {
                    let m: serde_json::Value = serde_json::from_str(&manifest_str)?;
                    let ver_str = m
                        .get("export_version")
                        .and_then(|v| v.as_str())
                        .unwrap_or("0.0.0");
                    let ver = parse_semver(ver_str);
                    info!(
                        "Import archive manifest: export_version={}.{}.{}",
                        ver.0, ver.1, ver.2
                    );
                    ver
                }
                Err(_) => {
                    info!("No manifest.json found — treating as legacy (0.0.0) export");
                    (0, 0, 0)
                }
            };

            // 1. Read profile.json — parse as Value, migrate, then deserialize
            let profile_json = read_from_zip("profile.json")?;
            let mut profile_value: serde_json::Value = serde_json::from_str(&profile_json)?;
            migrate_profile_json(&mut profile_value, export_version)?;
            let mut profile: TaxpayerProfile = serde_json::from_value(profile_value)?;
            let tin = profile.tin.full();
            // The file's row id is never used. It names a row in the
            // database that wrote the file; here it can be another
            // taxpayer's row, which the save would overwrite, re-keying that
            // taxpayer's records onto this TIN. It comes from this database's
            // row with the same TIN, or is none when there is no such row.
            profile.id = db.get_profile_by_tin(&tin)?.and_then(|stored| stored.id);
            db.save_profile(profile)?;

            // 2. Read submissions.json — deduplicate via (tin, form_type, period, submitted_at)
            if let Ok(submissions_json) = read_from_zip("submissions.json") {
                let mut submissions: Vec<serde_json::Value> =
                    serde_json::from_str(&submissions_json)?;
                for sub_value in &mut submissions {
                    migrate_submission_json(sub_value, export_version)?;
                }
                let typed: Vec<Submission> = submissions
                    .into_iter()
                    .map(serde_json::from_value)
                    .collect::<Result<_, _>>()?;

                for sub in typed {
                    let json_data = serde_json::to_string(&sub.form_data)?;
                    let submitted_at_key = sub.submitted_at.clone().unwrap_or_default();

                    // Check if a matching submission already exists (using COALESCE to match the index)
                    let exists: bool = db
                        .conn
                        .query_row(
                            "SELECT 1 FROM submissions
                     WHERE tin = ?1 AND form_type = ?2 AND period = ?3
                       AND COALESCE(submitted_at, '') = ?4",
                            rusqlite::params![tin, sub.form_type, sub.period, submitted_at_key],
                            |_| Ok(true),
                        )
                        .unwrap_or(false);

                    if exists {
                        db.conn.execute(
                            "UPDATE submissions SET status = ?1, form_data = ?2, filename = ?3,
                                updated_at = datetime('now')
                         WHERE tin = ?4 AND form_type = ?5 AND period = ?6
                           AND COALESCE(submitted_at, '') = ?7",
                            rusqlite::params![
                                sub.status,
                                json_data,
                                sub.filename,
                                tin,
                                sub.form_type,
                                sub.period,
                                submitted_at_key
                            ],
                        )?;
                    } else {
                        db.conn.execute(
                        "INSERT INTO submissions (tin, form_type, period, status, form_data, submitted_at, filename)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                        rusqlite::params![
                            tin, sub.form_type, sub.period, sub.status,
                            json_data, sub.submitted_at, sub.filename
                        ],
                    )?;
                    }
                }
            }

            // 3. Read drafts.json
            if let Ok(drafts_json) = read_from_zip("drafts.json") {
                let drafts: Vec<serde_json::Value> = serde_json::from_str(&drafts_json)?;

                for draft in drafts {
                    if let (Some(form_code), Some(taxable_year), Some(status), Some(data_json)) = (
                        draft.get("form_code").and_then(|v| v.as_str()),
                        draft.get("taxable_year").and_then(|v| v.as_i64()),
                        draft.get("status").and_then(|v| v.as_str()),
                        draft.get("data_json").and_then(|v| v.as_str()),
                    ) {
                        let quarter = draft.get("quarter").and_then(|v| v.as_i64());

                        db.conn.execute(
                        "INSERT INTO form_drafts (tin, form_code, taxable_year, quarter, status, data_json)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                         ON CONFLICT(tin, form_code, taxable_year, quarter)
                         DO UPDATE SET status = excluded.status,
                                       data_json = excluded.data_json,
                                       updated_at = datetime('now')",
                        rusqlite::params![
                            tin,
                            form_code,
                            taxable_year,
                            quarter,
                            status,
                            data_json
                        ],
                    )?;
                    }
                }
            }

            // 4. Read receipts
            if let Ok(manifest_json) = read_from_zip("receipts_manifest.json") {
                let manifest: Vec<String> = serde_json::from_str(&manifest_json)?;

                for filename in manifest {
                    let txt_name = format!("Receipts/{}.txt", filename);
                    let html_name = format!("Receipts/{}.html", filename);

                    if let Ok(raw_text) = read_from_zip(&txt_name) {
                        let raw_html = read_from_zip(&html_name).ok();

                        let mut form_type = "Unknown".to_string();
                        let mut period = "Unknown".to_string();
                        let received_date = "Unknown".to_string();
                        let received_time = "Unknown".to_string();

                        if let Some(split) = crate::receipt::split_bir_filename(&filename) {
                            form_type = split.1;
                            period = split.2;
                        }

                        let _ = db.conn.execute(
                        "INSERT INTO submission_receipts (filename, tin, form_type, period, received_date, received_time, source_from, raw_text, raw_html)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                         ON CONFLICT(filename) DO UPDATE SET raw_text = excluded.raw_text, raw_html = excluded.raw_html",
                        rusqlite::params![
                            filename,
                            tin,
                            form_type,
                            period,
                            received_date,
                            received_time,
                            "Import",
                            raw_text,
                            raw_html
                        ],
                    );
                    }
                }
            }
        }

        Ok(())
    })();
    match result {
        Ok(()) => {
            db.conn.execute_batch("COMMIT")?;
            Ok(())
        }
        Err(error) => {
            let _ = db.conn.execute_batch("ROLLBACK");
            Err(error)
        }
    }
}

pub fn extract_database_zip(zip_path: &Path, out_db_path: &Path) -> Result<(), DbError> {
    let file = fs::File::open(zip_path)?;
    let mut archive = ZipArchive::new(file).map_err(|e| DbError::Other(e.to_string()))?;

    let mut db_file_in_zip = archive
        .by_name("bir_data.db")
        .map_err(|e| DbError::Other(e.to_string()))?;

    // Extract to temp unencrypted db
    let temp_dir = std::env::temp_dir();
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_db_path = temp_dir.join(format!("bir_unencrypted_import_{}.db", timestamp));

    let mut temp_file = fs::File::create(&temp_db_path)?;
    std::io::copy(&mut db_file_in_zip, &mut temp_file)?;

    // Delete the current database so we can cleanly export into a new one
    if out_db_path.exists() {
        let _ = fs::remove_file(out_db_path);
    }
    let wal_path = out_db_path.with_extension("db-wal");
    let shm_path = out_db_path.with_extension("db-shm");
    if wal_path.exists() {
        let _ = fs::remove_file(wal_path);
    }
    if shm_path.exists() {
        let _ = fs::remove_file(shm_path);
    }

    // Open the unencrypted db
    let conn = rusqlite::Connection::open(&temp_db_path)?;
    let key_hex = Database::get_or_create_master_key()?;

    // Attach the target db (encrypted) and export to it
    conn.execute(
        &format!("ATTACH DATABASE ?1 AS encrypted KEY \"x'{}'\";", key_hex),
        rusqlite::params![out_db_path.to_str().unwrap()],
    )?;
    let mut stmt = conn.prepare("SELECT sqlcipher_export('encrypted');")?;
    let _ = stmt.query([])?.next()?;
    drop(stmt);
    conn.execute("DETACH DATABASE encrypted;", [])?;

    // Explicitly close the connection to ensure all data is flushed
    drop(conn);

    // Small delay to ensure file system operations complete
    std::thread::sleep(std::time::Duration::from_millis(100));

    // Clean up
    let _ = fs::remove_file(&temp_db_path);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Database, Submission};
    use crate::profile::{TaxpayerProfile, TaxpayerType};
    use tempfile::tempdir;

    #[test]
    fn test_profile_export_import_zip() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let (db, _) = Database::open_or_recreate(&db_path).unwrap();

        // Create mock profile
        let profile = TaxpayerProfile {
            id: None,
            full_name: "John Doe".to_string(),
            tin: crate::naming::Tin {
                segment1: "123".into(),
                segment2: "456".into(),
                segment3: "789".into(),
                branch: "000".into(),
            },
            rdo_code: "123".to_string(),
            line_of_business: "Tech".to_string(),
            registered_address: "123 Main St".to_string(),
            zip_code: "1000".to_string(),
            phone: "09123456789".to_string(),
            email: "test@example.com".to_string(),
            default_form_type: "2551Q".to_string(),
            taxpayer_type: TaxpayerType::Individual,
            is_vat_registered: false,
            business_start_date: None,
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
        db.save_profile(profile.clone()).unwrap();

        // Create mock submission
        let sub = Submission {
            id: None,
            tin: "123456789000".to_string(),
            form_type: "2551Q".to_string(),
            period: "2024Q1".to_string(),
            status: "Draft".to_string(),
            form_data: std::collections::BTreeMap::new(),
            submitted_at: None,
            filename: None,
            created_at: None,
            updated_at: None,
        };
        db.save_submission(sub.clone()).unwrap();

        // Export to zip
        let export_zip = dir.path().join("export.zip");
        crate::export::export_profile_data(&db, "123456789000", &export_zip).unwrap();
        assert!(export_zip.exists());

        // Import into new DB
        let db2_path = dir.path().join("test2.db");
        let (db2, _) = Database::open_or_recreate(&db2_path).unwrap();
        import_profile_data(&db2, &export_zip).unwrap();

        // Verify
        let imported_profile = db2.get_profile_by_tin("123456789000").unwrap().unwrap();
        assert_eq!(imported_profile.full_name, "John Doe".to_string());

        let submissions = db2.list_submissions_for_tin("123456789000").unwrap();
        assert_eq!(submissions.len(), 1);
        assert_eq!(submissions[0].form_type, "2551Q");
    }

    #[test]
    fn test_database_export_import_zip() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("original.db");
        let (db, _) = Database::open_or_recreate(&db_path).unwrap();

        let profile = TaxpayerProfile {
            id: None,
            full_name: "Jane Doe".to_string(),
            tin: crate::naming::Tin {
                segment1: "999".into(),
                segment2: "999".into(),
                segment3: "999".into(),
                branch: "000".into(),
            },
            rdo_code: "123".to_string(),
            line_of_business: "Tech".to_string(),
            registered_address: "123 Main St".to_string(),
            zip_code: "1000".to_string(),
            phone: "09123456789".to_string(),
            email: "test2@example.com".to_string(),
            default_form_type: "2551Q".to_string(),
            taxpayer_type: TaxpayerType::Individual,
            is_vat_registered: false,
            business_start_date: None,
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
        db.save_profile(profile).unwrap();
        db.checkpoint().unwrap();

        let backup_zip = dir.path().join("backup.db.zip");
        crate::export::export_database_zip(&db, &backup_zip).unwrap();
        assert!(backup_zip.exists());

        let restored_db_path = dir.path().join("restored.db");
        extract_database_zip(&backup_zip, &restored_db_path).unwrap();

        let (db2, _) = Database::open_or_recreate(&restored_db_path).unwrap();
        let profiles = db2.list_profiles().unwrap();
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].tin.full(), "999999999000");
    }

    #[test]
    fn test_legacy_profile_json_deserializes_with_defaults() {
        // A minimal JSON without any of the newer #[serde(default)] fields
        let legacy_json = r#"{
            "full_name": "Legacy User",
            "tin": {"segment1":"111","segment2":"222","segment3":"333","branch":"000"},
            "rdo_code": "039",
            "line_of_business": "IT",
            "registered_address": "Manila",
            "zip_code": "1000",
            "phone": "0912",
            "email": "legacy@test.com",
            "default_form_type": "2551Q"
        }"#;

        // Parse as Value, run migration, then deserialize
        let mut value: serde_json::Value = serde_json::from_str(legacy_json).unwrap();
        migrate_profile_json(&mut value, (0, 0, 0)).unwrap();
        let profile: TaxpayerProfile = serde_json::from_value(value).unwrap();

        assert_eq!(profile.full_name, "Legacy User");
        assert_eq!(profile.tax_classification, None);
        assert!(!profile.has_8_percent_election(2026));
        assert!(!profile.is_archived);
        assert!(!profile.email_tracking_enabled);
    }

    #[test]
    fn test_import_idempotent_no_duplicate_submissions() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test_idem.db");
        let (db, _) = Database::open_or_recreate(&db_path).unwrap();

        let profile = TaxpayerProfile {
            id: None,
            full_name: "Idem User".to_string(),
            tin: crate::naming::Tin {
                segment1: "555".into(),
                segment2: "555".into(),
                segment3: "555".into(),
                branch: "000".into(),
            },
            rdo_code: "039".to_string(),
            line_of_business: "IT".to_string(),
            registered_address: "QC".to_string(),
            zip_code: "1103".to_string(),
            phone: "0999".to_string(),
            email: "idem@test.com".to_string(),
            default_form_type: "2551Q".to_string(),
            taxpayer_type: TaxpayerType::Individual,
            is_vat_registered: false,
            business_start_date: None,
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
        db.save_profile(profile).unwrap();

        let sub = Submission {
            id: None,
            tin: "555555555000".to_string(),
            form_type: "2551Q".to_string(),
            period: "2024Q1".to_string(),
            status: "Draft".to_string(),
            form_data: std::collections::BTreeMap::new(),
            submitted_at: None,
            filename: None,
            created_at: None,
            updated_at: None,
        };
        db.save_submission(sub).unwrap();

        // Export
        let export_zip = dir.path().join("idem_export.zip");
        crate::export::export_profile_data(&db, "555555555000", &export_zip).unwrap();

        // Import into a new DB twice
        let db2_path = dir.path().join("test_idem2.db");
        let (db2, _) = Database::open_or_recreate(&db2_path).unwrap();
        import_profile_data(&db2, &export_zip).unwrap();
        // Second import — should NOT create duplicate submissions
        import_profile_data(&db2, &export_zip).unwrap();

        let submissions = db2.list_submissions_for_tin("555555555000").unwrap();
        assert_eq!(
            submissions.len(),
            1,
            "Re-importing should not create duplicate submissions"
        );
    }

    #[test]
    fn test_parse_semver_valid() {
        assert_eq!(parse_semver("0.0.1"), (0, 0, 1));
        assert_eq!(parse_semver("1.2.3"), (1, 2, 3));
        assert_eq!(parse_semver("10.20.30"), (10, 20, 30));
    }

    #[test]
    fn test_parse_semver_edge_cases() {
        // Missing parts default to 0
        assert_eq!(parse_semver("1"), (1, 0, 0));
        assert_eq!(parse_semver("1.2"), (1, 2, 0));
        // Empty / garbage defaults to (0,0,0)
        assert_eq!(parse_semver(""), (0, 0, 0));
        assert_eq!(parse_semver("not-a-version"), (0, 0, 0));
        // Whitespace is trimmed
        assert_eq!(parse_semver("  1.2.3  "), (1, 2, 3));
    }

    #[test]
    fn test_manifest_contains_cargo_version() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test_manifest.db");
        let (db, _) = Database::open_or_recreate(&db_path).unwrap();

        let profile = TaxpayerProfile {
            id: None,
            full_name: "Manifest Test".to_string(),
            tin: crate::naming::Tin {
                segment1: "777".into(),
                segment2: "777".into(),
                segment3: "777".into(),
                branch: "000".into(),
            },
            rdo_code: "039".to_string(),
            line_of_business: "IT".to_string(),
            registered_address: "BGC".to_string(),
            zip_code: "1634".to_string(),
            phone: "0917".to_string(),
            email: "manifest@test.com".to_string(),
            default_form_type: "2551Q".to_string(),
            taxpayer_type: TaxpayerType::Individual,
            is_vat_registered: false,
            business_start_date: None,
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
        db.save_profile(profile).unwrap();

        // Export
        let export_zip = dir.path().join("manifest_test.zip");
        crate::export::export_profile_data(&db, "777777777000", &export_zip).unwrap();

        // Read the ZIP and extract manifest.json directly
        let file = std::fs::File::open(&export_zip).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let mut manifest_file = archive.by_name("manifest.json").unwrap();
        let mut manifest_str = String::new();
        manifest_file.read_to_string(&mut manifest_str).unwrap();

        let manifest: serde_json::Value = serde_json::from_str(&manifest_str).unwrap();

        // export_version should be the Cargo package version string
        let version = manifest.get("export_version").unwrap();
        assert!(version.is_string(), "export_version should be a string");
        assert_eq!(
            version.as_str().unwrap(),
            env!("CARGO_PKG_VERSION"),
            "export_version should match CARGO_PKG_VERSION"
        );

        // exported_at should be present
        assert!(manifest.get("exported_at").is_some());

        // Verify the version parses correctly
        let ver = parse_semver(version.as_str().unwrap());
        assert_eq!(ver, parse_semver(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn test_semver_comparison_ordering() {
        // Verify that tuple comparison works correctly for migration gates
        assert!((0, 0, 0) < (0, 0, 1));
        assert!((0, 0, 1) < (0, 1, 0));
        assert!((0, 1, 0) < (1, 0, 0));
        assert!((0, 0, 1) < (0, 2, 0));
        assert!((0, 0, 1) >= (0, 0, 1));
        assert!((1, 0, 0) < (1, 0, 1));
    }

    // ---------- an archive's row id is never used ----------

    use std::collections::BTreeMap;

    const TIN_Y: &str = "11122233300000";
    const TIN_X: &str = "44455566600000";

    /// A profile for the 14-digit `tin`.
    fn branch_profile(full_name: &str, tin: &str) -> TaxpayerProfile {
        serde_json::from_value(serde_json::json!({
            "id": null,
            "full_name": full_name,
            "tin": {
                "segment1": &tin[0..3],
                "segment2": &tin[3..6],
                "segment3": &tin[6..9],
                "branch": &tin[9..],
            },
            "rdo_code": "040",
            "line_of_business": "Retail",
            "registered_address": "1 Sample Street, Pasig",
            "zip_code": "1600",
            "phone": "0280000000",
            "email": "books@example.com",
            "default_form_type": "2551Q",
        }))
        .expect("profile fixture")
    }

    /// Write a zip archive holding `entries` as (name, contents).
    fn write_archive(path: &Path, entries: &[(&str, String)]) {
        let mut zip = zip::ZipWriter::new(fs::File::create(path).unwrap());
        for (name, contents) in entries {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            std::io::Write::write_all(&mut zip, contents.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }

    /// A crafted profile archive: `profile` with the row id `id`.
    fn crafted_archive(path: &Path, profile: &TaxpayerProfile, id: i64) {
        let mut record = serde_json::to_value(profile).unwrap();
        record["id"] = serde_json::json!(id);
        let manifest = serde_json::json!({ "export_version": env!("CARGO_PKG_VERSION") });
        write_archive(
            path,
            &[
                ("manifest.json", manifest.to_string()),
                ("profile.json", record.to_string()),
            ],
        );
    }

    /// An archive for a TIN this database has: the row keeps its own row
    /// id, whether the file names that row's id or one that names no row
    /// here, and takes the file's other fields.
    #[test]
    fn an_import_keeps_the_stored_rows_id() {
        let dir = tempdir().unwrap();
        let db = Database::open_ephemeral().unwrap();
        let stored = branch_profile("Stored Name", TIN_Y);
        let row_id = db.save_profile(stored).unwrap().id.expect("a row id");

        for (round, file_id) in [row_id, 4242].into_iter().enumerate() {
            let archive = dir.path().join(format!("crafted-{round}.zip"));
            let name = format!("Imported Name {round}");
            crafted_archive(&archive, &branch_profile(&name, TIN_Y), file_id);
            import_profile_data(&db, &archive).unwrap();

            let imported = db.get_profile_by_tin(TIN_Y).unwrap().expect("the row");
            assert_eq!(imported.id, Some(row_id), "the row keeps its id");
            assert_eq!(
                imported.full_name, name,
                "the file's other fields are stored"
            );
        }
        assert_eq!(db.list_profiles().unwrap().len(), 1, "no second row");
    }

    /// The regression test for a data-corruption bug. The file's `id`
    /// reached `save_profile`, which reads it as this database's row: here
    /// row 1 is taxpayer Y's, so importing taxpayer X's archive that says
    /// `"id": 1` overwrote Y's profile with X's and re-keyed Y's draft and
    /// submission onto X's TIN. Y must keep its row, draft and submission,
    /// and X must get a row of its own.
    #[test]
    fn an_archive_row_id_never_names_another_taxpayers_row() {
        let dir = tempdir().unwrap();
        let db = Database::open_ephemeral().unwrap();
        let y = db
            .save_profile(branch_profile("Taxpayer Y", TIN_Y))
            .unwrap();
        assert_eq!(y.id, Some(1), "Y holds row 1");
        drop(y);
        let submission = |tin: &str, form_type: &str, period: &str, owner: &str| Submission {
            id: None,
            tin: tin.into(),
            form_type: form_type.into(),
            period: period.into(),
            status: "Draft".into(),
            form_data: BTreeMap::from([("owner".to_string(), owner.to_string())]),
            submitted_at: None,
            filename: None,
            created_at: None,
            updated_at: None,
        };
        db.save_submission(submission(TIN_Y, "2551Q", "2026Q1", "Y"))
            .unwrap();
        db.save_form_draft(
            TIN_Y,
            "1701Q",
            2026,
            Some(1),
            &crate::forms::FilingStatus::Draft,
            &serde_json::json!({ "owner": "Y" }),
        )
        .unwrap();

        let archive = dir.path().join("taxpayer-x.zip");
        let mut x = serde_json::to_value(branch_profile("Taxpayer X", TIN_X)).unwrap();
        x["id"] = serde_json::json!(1);
        let x_submissions = [submission(TIN_X, "2550Q", "2026Q2", "X")];
        write_archive(
            &archive,
            &[
                ("profile.json", x.to_string()),
                (
                    "submissions.json",
                    serde_json::to_string(&x_submissions).unwrap(),
                ),
            ],
        );
        import_profile_data(&db, &archive).unwrap();

        let y = db
            .get_profile_by_tin(TIN_Y)
            .unwrap()
            .expect("Y's profile is still there");
        assert_eq!(y.id, Some(1), "Y keeps row 1");
        assert_eq!(y.full_name, "Taxpayer Y");
        let x = db
            .get_profile_by_tin(TIN_X)
            .unwrap()
            .expect("X is imported");
        assert!(x.id.is_some_and(|id| id != 1), "X gets a row of its own");
        assert_eq!(x.full_name, "Taxpayer X");
        assert_eq!(db.list_profiles().unwrap().len(), 2);

        let owners = |tin: &str| -> Vec<String> {
            db.list_submissions_for_tin(tin)
                .unwrap()
                .into_iter()
                .map(|submission| submission.form_data["owner"].clone())
                .collect()
        };
        assert_eq!(owners(TIN_Y), ["Y"], "Y's submission stays Y's");
        assert_eq!(owners(TIN_X), ["X"], "X has only its own submission");
        let draft = |tin: &str| -> Option<serde_json::Value> {
            db.get_form_draft(tin, "1701Q", 2026, Some(1)).unwrap()
        };
        assert_eq!(draft(TIN_Y).expect("Y's draft stays Y's")["owner"], "Y");
        assert!(draft(TIN_X).is_none(), "X has none of Y's drafts");
    }

    /// Export, then import into a fresh database: every profile field
    /// matches except the row id, which is the new database's own.
    #[test]
    fn a_round_trip_keeps_every_profile_field_but_the_id() {
        use crate::profile::{
            ComplianceSourceMode, EoptTier, ExciseTaxCategory, IncomeTaxElection,
            TaxClassification, TaxElectionHistory,
        };
        let date = |year, month, day| chrono::NaiveDate::from_ymd_opt(year, month, day).unwrap();
        let dir = tempdir().unwrap();
        let source = Database::open_ephemeral().unwrap();
        // Row 1 is another taxpayer's, so the exported profile is row 2: not
        // the id a fresh database gives the import.
        source
            .save_profile(branch_profile("Earlier Taxpayer", TIN_Y))
            .unwrap();
        let mut profile = branch_profile("Round Trip Corp", TIN_X);
        profile.is_vat_registered = true;
        profile.business_start_date = Some(date(2020, 1, 15));
        profile.birth_date = Some(date(1985, 6, 30));
        profile.tax_classification = Some(TaxClassification::SelfEmployed);
        profile.eopt_tier = Some(EoptTier::Small);
        profile.is_bmbe = true;
        profile.is_create_msme = true;
        profile.is_expanded_withholding_agent = true;
        profile.atc_codes = vec!["WI010".into(), "WC010".into()];
        profile.excise_tax_categories = vec![ExciseTaxCategory::Alcohol];
        profile.tax_elections = vec![TaxElectionHistory {
            taxable_year: 2026,
            election: IncomeTaxElection::EightPercent,
            elected_at: date(2026, 2, 1).and_hms_opt(9, 30, 0).unwrap(),
            source_form: "2551Q".into(),
        }];
        profile.email_tracking_enabled = true;
        profile.test_notification_enabled = true;
        profile.has_employees = true;
        profile.withholds_compensation = true;
        profile.withholds_expanded = true;
        profile.withholds_final = true;
        profile.is_top_withholding_agent = true;
        profile.compliance_source_mode = ComplianceSourceMode::TemporalSuggestion;
        assert_eq!(source.save_profile(profile).unwrap().id, Some(2));

        let archive = dir.path().join("round-trip.zip");
        crate::export::export_profile_data(&source, TIN_X, &archive).unwrap();
        let target = Database::open_ephemeral().unwrap();
        import_profile_data(&target, &archive).unwrap();

        let comparable = |db: &Database| -> serde_json::Value {
            let profile = db.get_profile_by_tin(TIN_X).unwrap().expect("the profile");
            let mut record = serde_json::to_value(&profile).unwrap();
            record.as_object_mut().unwrap().remove("id");
            record
        };
        assert_eq!(
            comparable(&target),
            comparable(&source),
            "a profile field changed on the round trip"
        );
        let imported = target.get_profile_by_tin(TIN_X).unwrap().unwrap();
        assert_eq!(imported.id, Some(1), "the import is this database's row");
    }

    /// The Settings all-profiles backup puts each profile under
    /// `Profiles/<TIN>/`: the import reads every one, not just a top-level
    /// `profile.json`.
    #[test]
    fn a_combined_archive_imports_every_profile() {
        let dir = tempdir().unwrap();
        let db = Database::open_ephemeral().unwrap();
        let archive = dir.path().join("all-profiles.zip");
        write_archive(
            &archive,
            &[
                (
                    &format!("Profiles/{TIN_Y}/profile.json"),
                    serde_json::to_string(&branch_profile("Combined One", TIN_Y)).unwrap(),
                ),
                (
                    &format!("Profiles/{TIN_X}/profile.json"),
                    serde_json::to_string(&branch_profile("Combined Two", TIN_X)).unwrap(),
                ),
            ],
        );

        import_profile_data(&db, &archive).unwrap();
        let mut names: Vec<String> = db
            .list_profiles()
            .unwrap()
            .iter()
            .map(|profile| profile.full_name.clone())
            .collect();
        names.sort();
        assert_eq!(names, ["Combined One", "Combined Two"]);
    }

    /// The whole archive writes under one transaction: a later profile's
    /// bad `profile.json` fails the import and rolls the earlier profile
    /// back — no half-imported state.
    #[test]
    fn a_failed_second_profile_rolls_back_the_whole_import() {
        let dir = tempdir().unwrap();
        let db = Database::open_ephemeral().unwrap();
        let archive = dir.path().join("broken.zip");
        // The valid profile comes first in the archive, so without the
        // transaction it would already be saved when the second fails.
        write_archive(
            &archive,
            &[
                (
                    &format!("Profiles/{TIN_Y}/profile.json"),
                    serde_json::to_string(&branch_profile("Rolled Back", TIN_Y)).unwrap(),
                ),
                (
                    &format!("Profiles/{TIN_X}/profile.json"),
                    "{ not json".to_string(),
                ),
            ],
        );

        assert!(import_profile_data(&db, &archive).is_err());
        assert!(
            db.list_profiles().unwrap().is_empty(),
            "the first profile survived a rolled-back import"
        );
    }
}
