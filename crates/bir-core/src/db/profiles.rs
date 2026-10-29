//! Profile repository — CRUD for TaxpayerProfile.

use rusqlite::{Connection, OptionalExtension, params};

use super::{Database, DbError};
use crate::profile::{
    TaxProfileVersion, TaxProfileVersionConfirmationPlan, TaxProfileVersionSource,
    TaxProfileVersionStatus, TaxpayerProfile,
};

/// See [`Database::confirm_initial_profile_year`]. `conn` may be an open
/// transaction on the profiles connection.
pub(super) fn confirm_initial_profile_year_on(
    conn: &Connection,
    tin: &str,
    year: i32,
) -> Result<bool, DbError> {
    let json: Option<String> = conn
        .query_row(
            "SELECT data_json FROM profiles WHERE tin = ?1",
            params![tin],
            |row| row.get(0),
        )
        .optional()?;
    let Some(json) = json else {
        return Ok(false);
    };
    let mut profile: TaxpayerProfile = serde_json::from_str(&json)?;
    profile.normalize_profile_version_review_statuses();
    if !profile.confirmed_profile_versions().is_empty() {
        return Ok(false);
    }
    let healable = profile.profile_versions.is_empty()
        || (profile.profile_versions.len() == 1
            && profile.profile_versions[0].source == TaxProfileVersionSource::MigrationBackfill
            && profile.profile_versions[0].status != TaxProfileVersionStatus::Confirmed);
    if !healable {
        return Ok(false);
    }
    if profile.business_start_date.is_none() {
        let start = chrono::NaiveDate::from_ymd_opt(year, 1, 1)
            .ok_or_else(|| DbError::Other(format!("{year} is not a calendar year")))?;
        profile.business_start_date = Some(start);
    }
    profile.profile_versions.clear();
    profile.ensure_profile_version_ledger();
    let stored = serde_json::to_string(&profile)?;
    conn.execute(
        "UPDATE profiles SET data_json = ?1 WHERE tin = ?2",
        params![stored, tin],
    )?;
    Ok(true)
}

fn reconciliation_calendar_year(local_date: chrono::NaiveDate) -> u16 {
    use chrono::Datelike as _;

    local_date.year() as u16
}

fn same_persisted_version_except_label(
    stored: &TaxProfileVersion,
    submitted: &TaxProfileVersion,
) -> bool {
    let mut stored = stored.clone();
    let mut submitted = submitted.clone();
    stored.label.clear();
    submitted.label.clear();

    let Ok(stored) = serde_json::to_value(stored) else {
        return false;
    };
    let Ok(submitted) = serde_json::to_value(submitted) else {
        return false;
    };
    stored == submitted
}

fn is_exact_initial_migration_backfill(
    profile: &TaxpayerProfile,
    version: &TaxProfileVersion,
) -> bool {
    version.source == TaxProfileVersionSource::MigrationBackfill
        && same_persisted_version_except_label(
            &TaxProfileVersion::from_profile_backfill(profile),
            version,
        )
}

fn validate_reviewed_confirmation_plan(
    existing_profile: Option<&TaxpayerProfile>,
    submitted_profile: &TaxpayerProfile,
    reviewed_plan: Option<&TaxProfileVersionConfirmationPlan>,
) -> Result<(), DbError> {
    let Some(existing_profile) = existing_profile else {
        let confirmed = submitted_profile
            .profile_versions
            .iter()
            .filter(|version| version.status == TaxProfileVersionStatus::Confirmed)
            .collect::<Vec<_>>();
        let Some(reviewed_plan) = reviewed_plan else {
            if confirmed.is_empty()
                || (confirmed.len() == 1
                    && is_exact_initial_migration_backfill(submitted_profile, confirmed[0]))
            {
                return Ok(());
            }
            return Err(DbError::Other(
                "A new confirmed COR or manual profile version requires an explicit reviewed confirmation plan; only the exact one-time migration backfill may be generated automatically"
                    .to_string(),
            ));
        };

        if confirmed.iter().any(|version| {
            version.id != reviewed_plan.version_id
                && version.source != TaxProfileVersionSource::MigrationBackfill
        }) {
            return Err(DbError::Other(
                "The reviewed confirmation plan authorizes only one new confirmed profile version"
                    .to_string(),
            ));
        }

        let mut planning_profile = submitted_profile.clone();
        let Some(target) = planning_profile
            .profile_versions
            .iter_mut()
            .find(|version| version.id == reviewed_plan.version_id)
        else {
            return Err(DbError::Other(
                "The submitted profile does not match the reviewed confirmation plan".to_string(),
            ));
        };
        if target.status != TaxProfileVersionStatus::Confirmed
            || target.effective_from != Some(reviewed_plan.effective_from)
            || target.effective_until.is_some()
        {
            return Err(DbError::Other(
                "The submitted profile does not match the reviewed confirmation plan".to_string(),
            ));
        }
        target.status = TaxProfileVersionStatus::Draft;

        for closure in &reviewed_plan.auto_close_consequences {
            let Some(prior) = planning_profile
                .profile_versions
                .iter_mut()
                .find(|version| version.id == closure.version_id)
            else {
                return Err(DbError::Other(
                    "The submitted profile does not match the reviewed confirmation plan"
                        .to_string(),
                ));
            };
            if prior.status != TaxProfileVersionStatus::Confirmed
                || prior.effective_from != closure.effective_from
                || prior.effective_until != Some(closure.effective_until)
            {
                return Err(DbError::Other(
                    "The submitted profile does not match the reviewed confirmation plan"
                        .to_string(),
                ));
            }
            prior.effective_until = None;
        }

        let expected_plan = planning_profile
            .profile_version_confirmation_plan(
                &reviewed_plan.version_id,
                reviewed_plan.effective_from,
            )
            .ok_or_else(|| {
                DbError::Other(
                    "The submitted profile does not match the reviewed confirmation plan"
                        .to_string(),
                )
            })?;
        if &expected_plan != reviewed_plan {
            return Err(DbError::Other(
                "The submitted profile does not match the exact reviewed confirmation plan"
                    .to_string(),
            ));
        }

        for version in confirmed {
            if version.id == reviewed_plan.version_id {
                continue;
            }
            let Some(expected_backfill) = planning_profile
                .profile_versions
                .iter()
                .find(|candidate| candidate.id == version.id)
            else {
                return Err(DbError::Other(
                    "The submitted profile contains an unreviewed confirmed profile version"
                        .to_string(),
                ));
            };
            if !is_exact_initial_migration_backfill(submitted_profile, expected_backfill) {
                return Err(DbError::Other(
                    "The submitted profile contains an unreviewed confirmed profile version"
                        .to_string(),
                ));
            }
        }
        return Ok(());
    };

    let newly_confirmed = submitted_profile
        .profile_versions
        .iter()
        .filter(|version| {
            version.status == TaxProfileVersionStatus::Confirmed
                && existing_profile
                    .profile_versions
                    .iter()
                    .find(|stored| stored.id == version.id)
                    .is_none_or(|stored| stored.status != TaxProfileVersionStatus::Confirmed)
        })
        .collect::<Vec<_>>();
    if newly_confirmed.len() > 1 {
        return Err(DbError::Other(
            "Confirm profile versions one at a time so each timeline change can be reviewed"
                .to_string(),
        ));
    }

    let mut authorized_timeline = existing_profile.clone();
    let authorized_target_id = if let Some(version) = newly_confirmed.first() {
        let reviewed_plan = reviewed_plan.ok_or_else(|| {
            DbError::Other(
                "Confirming a persisted profile version: an explicit reviewed confirmation plan is required"
                    .to_string(),
            )
        })?;
        let effective_from = version.effective_from.ok_or_else(|| {
            DbError::Other(
                "A confirmed profile version must have an effective start date".to_string(),
            )
        })?;

        if let Some(stored_version) = authorized_timeline
            .profile_versions
            .iter()
            .find(|stored| stored.id == version.id)
        {
            if stored_version.status == TaxProfileVersionStatus::Archived {
                return Err(DbError::Other(
                    "Archived profile versions cannot be confirmed directly; create a replacement version through the reviewed confirmation workflow"
                        .to_string(),
                ));
            }
            if stored_version
                .effective_from
                .or(stored_version.cor.registration_date)
                != Some(effective_from)
            {
                return Err(DbError::Other(
                    "The profile timeline changed after confirmation was reviewed; review the consequence again"
                        .to_string(),
                ));
            }
        } else {
            authorized_timeline
                .profile_versions
                .push((*version).clone());
        }

        let expected_plan = authorized_timeline
            .profile_version_confirmation_plan(&version.id, effective_from)
            .ok_or_else(|| {
                DbError::Other(
                    "The reviewed profile-version confirmation plan is stale or does not match a newly confirmed version"
                        .to_string(),
                )
            })?;
        if &expected_plan != reviewed_plan {
            return Err(DbError::Other(
                "The profile timeline changed after confirmation was reviewed; review the consequence again"
                    .to_string(),
            ));
        }
        if !authorized_timeline.apply_profile_version_confirmation_plan(reviewed_plan) {
            return Err(DbError::Other(
                "The profile timeline changed after confirmation was reviewed; review the consequence again"
                    .to_string(),
            ));
        }

        Some(version.id.clone())
    } else {
        if reviewed_plan.is_some() {
            return Err(DbError::Other(
                "The reviewed profile-version confirmation plan is stale or does not match a newly confirmed version"
                    .to_string(),
            ));
        }
        None
    };

    for stored in existing_profile.profile_versions.iter().filter(|version| {
        matches!(
            version.status,
            TaxProfileVersionStatus::Confirmed | TaxProfileVersionStatus::Archived
        )
    }) {
        let authorized = authorized_timeline
            .profile_versions
            .iter()
            .find(|version| version.id == stored.id)
            .expect("the authorized timeline starts from the stored profile");
        let submitted = submitted_profile
            .profile_versions
            .iter()
            .find(|version| version.id == stored.id);
        if !submitted
            .is_some_and(|version| same_persisted_version_except_label(authorized, version))
        {
            return Err(DbError::Other(
                "Confirmed and archived profile-version facts cannot change; create a replacement version and confirm it through the reviewed workflow"
                    .to_string(),
            ));
        }
    }

    if let Some(target_id) = authorized_target_id {
        let authorized = authorized_timeline
            .profile_versions
            .iter()
            .find(|version| version.id == target_id)
            .expect("the reviewed target exists in the authorized timeline");
        let submitted = submitted_profile
            .profile_versions
            .iter()
            .find(|version| version.id == target_id);
        if !submitted
            .is_some_and(|version| same_persisted_version_except_label(authorized, version))
        {
            return Err(DbError::Other(
                "The submitted profile version contains changes outside the exact reviewed confirmation plan"
                    .to_string(),
            ));
        }
    }

    Ok(())
}

/// Delete one branch profile and every row keyed by its TIN. The caller
/// owns the transaction and re-syncs `taxpayers`.
pub(super) fn delete_profile_rows(tx: &rusqlite::Connection, tin: &str) -> Result<(), DbError> {
    for (table, column) in [
        ("penalties_cache", "tin"),
        ("submissions", "tin"),
        ("legacy_filed_returns", "tin"),
        ("form_drafts", "tin"),
        ("submission_receipts", "tin"),
        ("data_providers", "profile_tin"),
        ("per_year_forms", "tin"),
        ("profile_calendar_events", "profile_tin"),
        ("form_templates", "tin"),
    ] {
        if table == "legacy_filed_returns" {
            tx.execute(
                &format!("DELETE FROM {table} WHERE {column} = ?1 OR {column} = ?2"),
                params![tin, canonical_legacy_filed_return_tin(tin)],
            )?;
        } else {
            tx.execute(
                &format!("DELETE FROM {table} WHERE {column} = ?1"),
                params![tin],
            )?;
        }
    }
    tx.execute("DELETE FROM profiles WHERE tin = ?1", params![tin])?;
    Ok(())
}

pub(super) fn canonical_legacy_filed_return_tin(tin: &str) -> String {
    crate::naming::Tin::parse_digits(tin)
        .map(|parsed| {
            format!(
                "{}{}{}{:05}",
                parsed.segment1,
                parsed.segment2,
                parsed.segment3,
                parsed.branch.parse::<u32>().unwrap_or_default()
            )
        })
        .unwrap_or_else(|| tin.to_string())
}

impl Database {
    fn migrate_profile_tin_references(
        conn: &rusqlite::Connection,
        old_tin: &str,
        new_tin: &str,
    ) -> Result<(), DbError> {
        if old_tin == new_tin {
            return Ok(());
        }

        for (table, column) in [
            ("penalties_cache", "tin"),
            ("submissions", "tin"),
            ("legacy_filed_returns", "tin"),
            ("form_drafts", "tin"),
            ("submission_receipts", "tin"),
            ("data_providers", "profile_tin"),
            ("per_year_forms", "tin"),
            ("profile_calendar_events", "profile_tin"),
            ("profile_calendar_links", "profile_tin"),
            ("form_templates", "tin"),
        ] {
            if table == "legacy_filed_returns" {
                conn.execute(
                    &format!("UPDATE {table} SET {column} = ?1 WHERE {column} = ?2"),
                    params![
                        canonical_legacy_filed_return_tin(new_tin),
                        canonical_legacy_filed_return_tin(old_tin)
                    ],
                )?;
            } else {
                conn.execute(
                    &format!("UPDATE {table} SET {column} = ?1 WHERE {column} = ?2"),
                    params![new_tin, old_tin],
                )?;
            }
        }
        Ok(())
    }

    pub fn save_profile(&self, profile: TaxpayerProfile) -> Result<TaxpayerProfile, DbError> {
        self.save_profile_with_post_commit_status(profile)
            .map(super::PostCommitWrite::into_committed)
    }

    /// Save a profile while keeping the post-commit calendar/deadline refresh
    /// status distinct from transaction success.
    pub fn save_profile_with_post_commit_status(
        &self,
        profile: TaxpayerProfile,
    ) -> Result<super::PostCommitWrite<TaxpayerProfile>, DbError> {
        self.save_profile_inner(profile, None)
    }

    pub fn save_profile_with_confirmation_plan(
        &self,
        profile: TaxpayerProfile,
        reviewed_plan: &TaxProfileVersionConfirmationPlan,
    ) -> Result<TaxpayerProfile, DbError> {
        self.save_profile_with_confirmation_plan_and_post_commit_status(profile, reviewed_plan)
            .map(super::PostCommitWrite::into_committed)
    }

    /// Save a reviewed profile-version confirmation while keeping a failed
    /// post-commit refresh request from masquerading as a rolled-back save.
    pub fn save_profile_with_confirmation_plan_and_post_commit_status(
        &self,
        profile: TaxpayerProfile,
        reviewed_plan: &TaxProfileVersionConfirmationPlan,
    ) -> Result<super::PostCommitWrite<TaxpayerProfile>, DbError> {
        self.save_profile_inner(profile, Some(reviewed_plan))
    }

    fn save_profile_inner(
        &self,
        mut profile: TaxpayerProfile,
        reviewed_plan: Option<&TaxProfileVersionConfirmationPlan>,
    ) -> Result<super::PostCommitWrite<TaxpayerProfile>, DbError> {
        profile.ensure_profile_version_ledger();
        let tin = profile.tin.full();
        let previous_tin = profile
            .id
            .map(|id| {
                self.conn
                    .query_row(
                        "SELECT tin FROM profiles WHERE id = ?1",
                        params![id],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()
            })
            .transpose()?
            .flatten();
        let lookup_tin = previous_tin.as_deref().unwrap_or(&tin);

        let existing_profile = self.get_profile(lookup_tin)?;
        if profile.atc_codes.is_empty()
            && let Some(existing_atc_codes) = existing_profile
                .as_ref()
                .map(|stored| &stored.atc_codes)
                .filter(|codes| !codes.is_empty())
        {
            profile.atc_codes.clone_from(existing_atc_codes);
        }

        validate_reviewed_confirmation_plan(existing_profile.as_ref(), &profile, reviewed_plan)?;
        profile
            .validate_confirmed_profile_timeline()
            .map_err(DbError::Other)?;

        // A caller may already hold a transaction — the profile import
        // writes the whole archive atomically. Then this save rides that
        // transaction (SQLite refuses a nested BEGIN) and only its owner
        // commits; alone, this save owns and commits its own transaction.
        let tx = self
            .conn
            .is_autocommit()
            .then(|| self.conn.unchecked_transaction())
            .transpose()?;
        let conn: &Connection = match &tx {
            Some(tx) => tx,
            None => &self.conn,
        };
        conn.execute_batch("PRAGMA defer_foreign_keys = ON;")?;

        let json_data = serde_json::to_string(&profile)?;

        if let Some(id) = profile.id {
            let updated = conn.execute(
                "UPDATE profiles SET tin = ?1, data_json = ?2 WHERE id = ?3",
                params![tin, json_data, id],
            )?;
            if updated == 0 {
                conn.execute(
                    "INSERT INTO profiles (tin, data_json) VALUES (?1, ?2)",
                    params![tin, json_data],
                )?;
                profile.id = Some(conn.last_insert_rowid());
            }
        } else if let Some(_existing) = self.get_profile(&tin)? {
            return Err(DbError::Sqlite(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ErrorCode::ConstraintViolation as i32),
                Some(format!("A profile with TIN {} already exists", tin)),
            )));
        } else {
            conn.execute(
                "INSERT INTO profiles (tin, data_json) VALUES (?1, ?2)",
                params![tin, json_data],
            )?;
            profile.id = Some(conn.last_insert_rowid());
        }
        if let Some(previous_tin) = previous_tin.as_deref() {
            Self::migrate_profile_tin_references(conn, previous_tin, &tin)?;
        }

        for (year, set) in &profile.per_year_forms {
            super::forms_set::execute_replace_per_year_forms(conn, &tin, *year, set)?;
        }

        // Refresh the current year and every already stored year in the same
        // transaction as the profile update. Ambiguous or undated timelines
        // preserve existing Forms Sets instead of guessing.
        let mut years_to_update = std::collections::BTreeSet::new();
        // Filing obligations follow the desktop user's local calendar year.
        // Using UTC here can reconcile the previous year during the first
        // local hours of January 1 in time zones east of UTC, while the UI and
        // emitted compliance event already identify the new local year.
        let current_year = reconciliation_calendar_year(chrono::Local::now().date_naive());
        years_to_update.insert(current_year);
        years_to_update.extend(profile.per_year_forms.keys().copied());
        if let Some(stored) = &existing_profile {
            years_to_update.extend(stored.per_year_forms.keys().copied());
        }

        for year in years_to_update {
            let resolved = profile.resolve_tax_profile_for_year(year);
            if resolved.has_blocking_issues() || resolved.effective_segments.is_empty() {
                continue;
            }

            let suggestions =
                crate::integration::validation::form_suggestions_for_profile_year(&profile, year);
            let existing_set = profile.per_year_forms.get(&year).or_else(|| {
                existing_profile
                    .as_ref()
                    .and_then(|stored| stored.per_year_forms.get(&year))
            });
            let result =
                crate::forms::reconcile_forms_set_for_year(year, existing_set, &suggestions);
            if !result.conflicts.is_empty() {
                tracing::warn!(
                    tin = %tin,
                    taxable_year = year,
                    conflicts = result.conflicts.len(),
                    "Forms Set reconciliation requires review"
                );
            }
            super::forms_set::execute_replace_per_year_forms(conn, &tin, year, &result.forms_set)?;
            profile.per_year_forms.insert(year, result.forms_set);
        }

        // Group a newly created (or re-keyed) profile under its taxpayer.
        super::taxpayers::sync_taxpayer_rows(conn)?;

        if let Some(tx) = tx {
            tx.commit()?;
        }
        Ok(self.finish_post_commit_write(profile, "Taxpayer profile save"))
    }

    /// Get a profile by TIN.
    pub fn get_profile(&self, tin: &str) -> Result<Option<TaxpayerProfile>, DbError> {
        let mut stmt = self
            .conn
            .prepare("SELECT data_json, id FROM profiles WHERE tin = ?1")?;
        let mut rows = stmt.query(params![tin])?;

        if let Some(row) = rows.next()? {
            let json_data: String = row.get(0)?;
            let mut profile: TaxpayerProfile = serde_json::from_str(&json_data)?;
            profile.normalize_profile_version_review_statuses();
            profile.id = row.get(1).ok();
            self.hydrate_profile_forms(&mut profile)?;
            Ok(Some(profile))
        } else {
            Ok(None)
        }
    }

    /// List all profiles.
    pub fn list_profiles(&self) -> Result<Vec<TaxpayerProfile>, DbError> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, data_json FROM profiles ORDER BY id ASC")?;
        let rows = stmt.query_map([], |row| {
            let id: i64 = row.get(0)?;
            let json_data: String = row.get(1)?;
            Ok((id, json_data))
        })?;

        let mut profiles = Vec::new();
        for row_result in rows {
            let (id, json_data) = row_result?;

            // Skip rows this build cannot read rather than failing the whole
            // list. A single unreadable row used to abort the query, so the
            // sidebar showed NO profiles at all while single-profile lookups
            // still succeeded - which is how a duplicate-TIN error could name a
            // profile the user had no way to see.
            //
            // The realistic cause is a database written by a newer build: an
            // enum gains a variant, an older binary meets it, and every profile
            // disappears. That is a downgrade, not corruption, and the other
            // rows are perfectly readable.
            let mut profile: TaxpayerProfile = match serde_json::from_str(&json_data) {
                Ok(profile) => profile,
                Err(error) => {
                    tracing::error!(
                        profile_id = id,
                        %error,
                        "Skipping a profile this build cannot deserialize; the remaining \
                         profiles are unaffected. This usually means the database was \
                         written by a newer version of the app."
                    );
                    // Application-wide rather than profile-scoped: the row is
                    // unreadable, so its TIN is precisely what we do not have.
                    // Errors here are swallowed - a profile listing must not
                    // fail because recording an alert about it failed.
                    const TITLE: &str = "A taxpayer profile could not be read";
                    let detail = format!(
                        "Profile #{id} was skipped and is not shown in the sidebar. \
                         This usually means the database was written by a newer version \
                         of the app. Details: {error}"
                    );
                    let _ = self.record_alert(
                        None,
                        super::alert_kinds::PROFILE_ROW_UNREADABLE,
                        super::AlertSeverity::Warning,
                        TITLE,
                        &detail,
                        super::AlertAction::OpenProfileManager,
                    );
                    continue;
                }
            };
            profile.normalize_profile_version_review_statuses();
            profile.id = Some(id);
            // Deliberately still fatal: a hydrate failure is a database-level
            // problem, not one row being from the future, and silently
            // returning profiles with missing forms would be worse than an error.
            self.hydrate_profile_forms(&mut profile)?;
            profiles.push(profile);
        }

        Ok(profiles)
    }

    /// Give a profile its first confirmed year when nothing is confirmed yet.
    ///
    /// A taxpayer created in the app stores no profile version and no business
    /// start, so Queue cannot resolve a period ("no confirmed taxpayer-profile
    /// version covers …"). This writes the one-time migration backfill
    /// directly into `profiles.data_json`: January 1 of `year` when the
    /// business start is unset, then one Confirmed version from that date.
    /// An existing business start is kept. A ledger that already has a
    /// confirmed version, or anything other than empty / one unconfirmed
    /// migration backfill, is left alone. Returns whether a version was written.
    pub fn confirm_initial_profile_year(&self, tin: &str, year: i32) -> Result<bool, DbError> {
        confirm_initial_profile_year_on(&self.conn, tin, year)
    }

    /// Get a single profile by its TIN.
    pub fn get_profile_by_tin(&self, tin: &str) -> Result<Option<TaxpayerProfile>, DbError> {
        self.get_profile(tin)
    }

    /// Delete a profile by TIN.
    pub fn delete_profile(&self, tin: &str) -> Result<(), DbError> {
        let tx = self.conn.unchecked_transaction()?;
        delete_profile_rows(&tx, tin)?;
        super::taxpayers::sync_taxpayer_rows(&tx)?;
        tx.commit()?;
        Ok(())
    }

    fn hydrate_profile_forms(&self, profile: &mut TaxpayerProfile) -> Result<(), DbError> {
        let tin = profile.tin.full();
        let years = self.list_forms_set_years(&tin)?;
        let mut per_year_forms = std::collections::BTreeMap::new();
        for y in years {
            let mut set = self.get_per_year_forms(&tin, y)?;
            // Enforce "one annual ITR per year" on already-stored data so a
            // legacy conflicting Forms Set surfaces a single active ITR
            // everywhere it is displayed; the healed set persists on the next
            // save. Idempotent for already-clean sets.
            let (keep_individual, keep_corporate) =
                crate::integration::primary_annual_itrs_for_profile_year(profile, y);
            set.deactivate_redundant_annual_itrs(keep_individual, keep_corporate);
            per_year_forms.insert(y, set);
        }
        profile.per_year_forms = per_year_forms;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{FixedOffset, TimeZone, Utc};
    use tempfile::NamedTempFile;

    fn listing_test_profile(seg1: &str) -> TaxpayerProfile {
        serde_json::from_value(serde_json::json!({
            "id": null,
            "full_name": format!("Taxpayer {seg1}"),
            "tin": { "segment1": seg1, "segment2": "456", "segment3": "789", "branch": "000" },
            "rdo_code": "018",
            "line_of_business": "Retail",
            "registered_address": "Manila",
            "zip_code": "1000",
            "phone": "09123456789",
            "email": "test@example.com",
            "default_form_type": "2551Qv2018",
            "taxpayer_type": "Individual",
            "business_start_date": "2020-01-01"
        }))
        .unwrap()
    }

    /// Reproduces the failure that hid every taxpayer profile in the sidebar.
    ///
    /// A row written by a newer build carried an enum variant this build does
    /// not know (`NeedsReview`, in the real incident). `list_profiles` used `?`
    /// on the per-row deserialize, so that single row aborted the whole query
    /// and the UI showed NO profiles - while single-profile lookups still
    /// succeeded, which is how a duplicate-TIN error could name a profile the
    /// user had no way to see.
    #[test]
    fn one_unreadable_row_does_not_hide_the_others() {
        let db = Database::open_in_memory_for_tests().expect("in-memory db");
        db.save_profile(listing_test_profile("111"))
            .expect("first profile saves");
        db.save_profile(listing_test_profile("222"))
            .expect("second profile saves");
        assert_eq!(db.list_profiles().unwrap().len(), 2, "precondition");

        // Rewrite exactly one row the way a downgrade does.
        db.conn
            .execute(
                "UPDATE profiles SET data_json = ?1 WHERE tin = ?2",
                params![
                    r#"{"full_name":"From The Future","status":"AVariantThisBuildLacks"}"#,
                    "222456789000"
                ],
            )
            .expect("row is rewritten");

        let listed = db
            .list_profiles()
            .expect("a single bad row must not fail the whole listing");

        assert_eq!(listed.len(), 1, "the readable profile must survive");
        assert_eq!(listed[0].tin.full(), "111456789000");
    }

    /// The skip must also become something the user can see. Logging alone is
    /// how this went unnoticed: profiles silently vanished from the sidebar
    /// while the log filled up where nobody was looking.
    #[test]
    fn skipping_a_row_raises_a_visible_alert() {
        let db = Database::open_in_memory_for_tests().expect("in-memory db");
        db.save_profile(listing_test_profile("111")).unwrap();
        db.conn
            .execute(
                "UPDATE profiles SET data_json = ?1",
                params![r#"{"status":"AVariantThisBuildLacks"}"#],
            )
            .unwrap();

        assert!(
            db.list_active_alerts(None).unwrap().is_empty(),
            "precondition: no alerts before the bad row is read"
        );

        let _ = db.list_profiles().expect("listing must not fail");

        let alerts = db.list_active_alerts(None).unwrap();
        assert_eq!(alerts.len(), 1, "the skip must surface as an alert");
        assert_eq!(
            alerts[0].kind,
            crate::db::alert_kinds::PROFILE_ROW_UNREADABLE
        );
        assert_eq!(alerts[0].action, crate::db::AlertAction::OpenProfileManager);
    }

    /// Listing runs constantly - the sidebar re-reads on every refresh - so the
    /// alert must not multiply.
    #[test]
    fn repeated_listings_do_not_multiply_the_alert() {
        let db = Database::open_in_memory_for_tests().expect("in-memory db");
        db.save_profile(listing_test_profile("111")).unwrap();
        db.conn
            .execute(
                "UPDATE profiles SET data_json = ?1",
                params![r#"{"status":"AVariantThisBuildLacks"}"#],
            )
            .unwrap();

        for _ in 0..25 {
            let _ = db.list_profiles().unwrap();
        }

        let alerts = db.list_active_alerts(None).unwrap();
        assert_eq!(alerts.len(), 1, "25 listings must not make 25 alerts");
        assert_eq!(alerts[0].occurrences, 25);
    }

    #[test]
    fn every_row_unreadable_yields_an_empty_list_not_an_error() {
        let db = Database::open_in_memory_for_tests().expect("in-memory db");
        db.save_profile(listing_test_profile("111")).unwrap();
        db.conn
            .execute(
                "UPDATE profiles SET data_json = ?1",
                params![r#"{"status":"AVariantThisBuildLacks"}"#],
            )
            .unwrap();

        assert!(
            db.list_profiles().expect("still must not error").is_empty(),
            "unreadable rows are skipped, so the list is empty rather than failed"
        );
    }

    #[test]
    fn reconciliation_calendar_year_uses_local_date_at_utc_positive_year_boundary() {
        let utc = Utc
            .with_ymd_and_hms(2026, 12, 31, 10, 30, 0)
            .single()
            .expect("valid UTC instant");
        let utc_plus_14 = FixedOffset::east_opt(14 * 60 * 60).expect("valid UTC+14 offset");
        let local_date = utc.with_timezone(&utc_plus_14).date_naive();

        assert_eq!(
            (utc.date_naive(), reconciliation_calendar_year(local_date)),
            (chrono::NaiveDate::from_ymd_opt(2026, 12, 31).unwrap(), 2027)
        );
    }

    #[test]
    fn profile_reads_do_not_synthesize_a_missing_version_ledger() {
        let file = NamedTempFile::new().unwrap();
        let db = Database::open(file.path()).unwrap();
        let legacy_json = serde_json::json!({
            "id": null,
            "full_name": "Post-migration legacy import",
            "tin": {
                "segment1": "444",
                "segment2": "555",
                "segment3": "666",
                "branch": "000"
            },
            "rdo_code": "039",
            "line_of_business": "Consulting",
            "registered_address": "Quezon City",
            "zip_code": "1100",
            "phone": "09170000100",
            "email": "profile@example.com",
            "default_form_type": "2551Q",
            "taxpayer_type": "Individual",
            "is_vat_registered": false,
            "business_start_date": "2020-04-15",
            "compliance_source_mode": "CorVersioned",
            "profile_versions": []
        })
        .to_string();
        db.conn
            .execute(
                "INSERT INTO profiles (tin, data_json) VALUES (?1, ?2)",
                rusqlite::params!["444555666000", legacy_json],
            )
            .unwrap();

        let loaded = db
            .get_profile("444555666000")
            .unwrap()
            .expect("profile should deserialize");
        assert!(loaded.profile_versions.is_empty());
        assert!(loaded.confirmed_profile_versions().is_empty());

        let listed = db.list_profiles().unwrap();
        assert_eq!(listed.len(), 1);
        assert!(listed[0].profile_versions.is_empty());
    }

    #[test]
    fn profile_save_round_trip_preserves_a_graduated_osd_election() {
        let file = NamedTempFile::new().unwrap();
        let db = Database::open(file.path()).unwrap();
        let profile: TaxpayerProfile = serde_json::from_value(serde_json::json!({
            "id": null,
            "full_name": "Election Persistence",
            "tin": {
                "segment1": "274",
                "segment2": "476",
                "segment3": "433",
                "branch": "00000"
            },
            "rdo_code": "018",
            "line_of_business": "Car Wash Services",
            "registered_address": "Castillejos",
            "zip_code": "2208",
            "phone": "09170000100",
            "email": "election@example.com",
            "default_form_type": "2551Qv2018",
            "taxpayer_type": "Individual",
            "tax_classification": "SelfEmployed",
            "is_vat_registered": false,
            "business_start_date": "2019-03-05",
            "tax_elections": [{
                "taxable_year": 2026,
                "election": "GraduatedOsd",
                "elected_at": "2026-01-01T00:00:00",
                "source_form": "profile_manager"
            }]
        }))
        .unwrap();

        let saved = db.save_profile(profile).unwrap();
        let reopened = db
            .get_profile(&saved.tin.full())
            .unwrap()
            .expect("saved profile should reopen");

        assert_eq!(
            (
                reopened.tax_elections.len(),
                &reopened.tax_elections[0].election
            ),
            (1, &crate::profile::IncomeTaxElection::GraduatedOsd)
        );
    }

    #[test]
    fn tin_reference_migration_moves_forms_records() {
        let file = NamedTempFile::new().unwrap();
        let db = Database::open(file.path()).unwrap();
        db.conn
            .execute(
                "INSERT INTO profiles (tin, data_json) VALUES (?1, '{}')",
                ["123456789000"],
            )
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO per_year_forms
                    (tin, taxable_year, form_code, frequency, active, source, custom)
                 VALUES (?1, 2026, '0619E', 'monthly', 1, 'manual', 0)",
                ["123456789000"],
            )
            .unwrap();

        let tx = db.conn.unchecked_transaction().unwrap();
        tx.execute_batch("PRAGMA defer_foreign_keys = ON;").unwrap();
        tx.execute(
            "UPDATE profiles SET tin = ?1 WHERE tin = ?2",
            params!["987654321000", "123456789000"],
        )
        .unwrap();
        Database::migrate_profile_tin_references(&tx, "123456789000", "987654321000").unwrap();
        tx.commit().unwrap();

        assert!(db.has_per_year_forms("987654321000", 2026).unwrap());
    }
}
