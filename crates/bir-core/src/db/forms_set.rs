//! Per-year Forms Set repository — CRUD for the `per_year_forms` table.
//!
//! The Forms Set is the user-owned, authoritative list of which BIR forms a taxpayer
//! files in a given taxable year (replacing the temporal suggestion engine).

use chrono::NaiveDate;
use rusqlite::{Connection, params};

use super::{Database, DbError};
use crate::forms::forms_set::{
    FormSetConflict, FormSetEntry, FormSetReviewStatus, FormSetSource, PerYearFormsSet,
};
use crate::forms::registry::FilingFrequency;

struct StoredFormSetEntry {
    form_code: String,
    frequency: String,
    active: i64,
    source: String,
    custom: i64,
    reason: Option<String>,
    source_reference: Option<String>,
    effective_from: Option<String>,
    effective_until: Option<String>,
    review_status: String,
    conflict_json: Option<String>,
}

pub(crate) fn frequency_to_str(f: &FilingFrequency) -> &'static str {
    match f {
        FilingFrequency::Quarterly => "quarterly",
        FilingFrequency::Annual => "annual",
        FilingFrequency::Monthly => "monthly",
        FilingFrequency::OpenEnded => "open_ended",
    }
}

fn frequency_from_str(s: &str) -> FilingFrequency {
    match s {
        "quarterly" => FilingFrequency::Quarterly,
        "annual" => FilingFrequency::Annual,
        "monthly" => FilingFrequency::Monthly,
        _ => FilingFrequency::OpenEnded,
    }
}

fn parse_optional_date(value: Option<String>, field: &str) -> Result<Option<NaiveDate>, DbError> {
    value
        .map(|value| {
            NaiveDate::parse_from_str(&value, "%Y-%m-%d").map_err(|error| {
                DbError::Other(format!(
                    "Invalid per_year_forms {field} date `{value}`: {error}"
                ))
            })
        })
        .transpose()
}

fn date_to_db(value: Option<NaiveDate>) -> Option<String> {
    value.map(|date| date.format("%Y-%m-%d").to_string())
}

pub(crate) fn execute_replace_per_year_forms(
    conn: &Connection,
    tin: &str,
    year: u16,
    set: &PerYearFormsSet,
) -> Result<(), DbError> {
    conn.execute(
        "DELETE FROM per_year_forms WHERE tin = ?1 AND taxable_year = ?2",
        params![tin, year],
    )?;
    for entry in &set.entries {
        let conflict_json = entry
            .conflict
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        conn.execute(
            "INSERT INTO per_year_forms
                (tin, taxable_year, form_code, frequency, active, source, custom, reason,
                 source_reference, effective_from, effective_until, review_status,
                 conflict_json, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                     datetime('now'))",
            params![
                tin,
                year,
                entry.form_code,
                frequency_to_str(&entry.frequency),
                entry.active as i64,
                entry.source.as_str(),
                entry.custom as i64,
                entry.reason,
                entry.source_reference,
                date_to_db(entry.effective_from),
                date_to_db(entry.effective_until),
                entry.review_status.as_str(),
                conflict_json,
            ],
        )?;
    }
    Ok(())
}

pub(crate) fn query_per_year_forms(
    conn: &Connection,
    tin: &str,
    year: u16,
) -> Result<PerYearFormsSet, DbError> {
    let mut stmt = conn.prepare(
        "SELECT form_code, frequency, active, source, custom, reason,
                source_reference, effective_from, effective_until, review_status,
                conflict_json
         FROM per_year_forms
         WHERE tin = ?1 AND taxable_year = ?2
         ORDER BY form_code ASC",
    )?;
    let rows = stmt.query_map(params![tin, year], |row| {
        Ok(StoredFormSetEntry {
            form_code: row.get(0)?,
            frequency: row.get(1)?,
            active: row.get(2)?,
            source: row.get(3)?,
            custom: row.get(4)?,
            reason: row.get(5)?,
            source_reference: row.get(6)?,
            effective_from: row.get(7)?,
            effective_until: row.get(8)?,
            review_status: row.get(9)?,
            conflict_json: row.get(10)?,
        })
    })?;

    let mut entries = Vec::new();
    for row in rows {
        let stored = row?;
        let conflict = stored
            .conflict_json
            .as_deref()
            .map(serde_json::from_str::<FormSetConflict>)
            .transpose()?;
        entries.push(FormSetEntry {
            form_code: stored.form_code,
            frequency: frequency_from_str(&stored.frequency),
            active: stored.active != 0,
            source: FormSetSource::from_str_lossy(&stored.source),
            custom: stored.custom != 0,
            reason: stored.reason,
            source_reference: stored.source_reference,
            effective_from: parse_optional_date(stored.effective_from, "effective_from")?,
            effective_until: parse_optional_date(stored.effective_until, "effective_until")?,
            review_status: FormSetReviewStatus::from_str_lossy(&stored.review_status),
            conflict,
        });
    }
    Ok(PerYearFormsSet {
        taxable_year: year,
        entries,
    })
}

impl Database {
    /// Load the Forms Set for `(tin, year)`. Returns an empty set if none is stored.
    pub fn get_per_year_forms(&self, tin: &str, year: u16) -> Result<PerYearFormsSet, DbError> {
        query_per_year_forms(&self.conn, tin, year)
    }

    /// Replace the entire Forms Set for `(tin, year)` (delete + insert, atomic).
    pub fn save_per_year_forms(
        &self,
        tin: &str,
        year: u16,
        set: &PerYearFormsSet,
    ) -> Result<(), DbError> {
        let tx = self.conn.unchecked_transaction()?;
        execute_replace_per_year_forms(&tx, tin, year, set)?;
        tx.commit()?;
        let _ = self.request_post_commit_refresh();
        Ok(())
    }

    /// Years (descending) that have a stored Forms Set for this TIN.
    pub fn list_forms_set_years(&self, tin: &str) -> Result<Vec<u16>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT taxable_year FROM per_year_forms
             WHERE tin = ?1 ORDER BY taxable_year DESC",
        )?;
        let rows = stmt.query_map(params![tin], |row| row.get::<_, i64>(0))?;
        let mut years = Vec::new();
        for row in rows {
            years.push(row? as u16);
        }
        Ok(years)
    }

    /// The authoritative list of form codes a taxpayer files in `year` — the active
    /// entries of the stored Forms Set. Empty when no set has been configured yet.
    pub fn active_form_codes_for_year(&self, tin: &str, year: u16) -> Result<Vec<String>, DbError> {
        Ok(self.get_per_year_forms(tin, year)?.active_form_codes())
    }

    /// Whether any Forms Set row exists for `(tin, year)`.
    pub fn has_per_year_forms(&self, tin: &str, year: u16) -> Result<bool, DbError> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM per_year_forms WHERE tin = ?1 AND taxable_year = ?2",
            params![tin, year],
            |r| r.get(0),
        )?;
        Ok(count > 0)
    }

    /// Delete the entire Forms Set for `(tin, year)`.
    pub fn delete_per_year_forms(&self, tin: &str, year: u16) -> Result<(), DbError> {
        self.conn.execute(
            "DELETE FROM per_year_forms WHERE tin = ?1 AND taxable_year = ?2",
            params![tin, year],
        )?;
        let _ = self.request_post_commit_refresh();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::forms::{FormSuggestion, FormSuggestionSource, reconcile_forms_set_for_year};
    use tempfile::NamedTempFile;

    #[test]
    fn per_year_forms_round_trip() {
        let db_file = NamedTempFile::new().unwrap();
        let db = match Database::open(db_file.path()) {
            Ok(db) => db,
            Err(e) => {
                println!("Skipping (keyring unavailable): {e:?}");
                return;
            }
        };

        let tin = "000234564000";

        // Empty by default.
        assert!(db.get_per_year_forms(tin, 2026).unwrap().is_empty());
        assert!(!db.has_per_year_forms(tin, 2026).unwrap());

        // Save a set with one suppressed entry.
        let mut set =
            PerYearFormsSet::from_codes(2026, ["2551Q", "1701Q", "1701"], FormSetSource::CorAi);
        set.entries[1].active = false;
        set.entries[1].reason = Some("filed annually instead".into());
        db.save_per_year_forms(tin, 2026, &set).unwrap();

        // Round-trip.
        let loaded = db.get_per_year_forms(tin, 2026).unwrap();
        assert_eq!(loaded.entries.len(), 3);
        assert!(loaded.contains_active("2551Q"));
        assert!(loaded.contains_active("1701"));
        assert!(!loaded.contains_active("1701Q"));
        assert_eq!(
            loaded.entry("1701Q").unwrap().reason.as_deref(),
            Some("filed annually instead")
        );
        assert_eq!(
            loaded.entry("2551Q").unwrap().frequency,
            FilingFrequency::Quarterly
        );

        // Per-year isolation.
        assert!(db.get_per_year_forms(tin, 2025).unwrap().is_empty());

        // Replace semantics: saving again overwrites.
        let set_2025 = PerYearFormsSet::from_codes(2025, ["0605"], FormSetSource::Manual);
        db.save_per_year_forms(tin, 2025, &set_2025).unwrap();
        assert_eq!(db.list_forms_set_years(tin).unwrap(), vec![2026, 2025]);

        // Delete.
        db.delete_per_year_forms(tin, 2026).unwrap();
        assert!(!db.has_per_year_forms(tin, 2026).unwrap());
        assert_eq!(db.list_forms_set_years(tin).unwrap(), vec![2025]);
    }

    #[test]
    fn per_year_forms_round_trip_preserves_provenance_and_conflicts() {
        let db_file = NamedTempFile::new().unwrap();
        let db = match Database::open(db_file.path()) {
            Ok(db) => db,
            Err(error) => {
                println!("Skipping (keyring unavailable): {error:?}");
                return;
            }
        };
        let tin = "000234564000";
        let mut reviewed = FormSuggestion::active("2551Q", FormSuggestionSource::ReviewedCor);
        reviewed.source_reference = Some("cor:sha256:reviewed".into());
        reviewed.effective_from = NaiveDate::from_ymd_opt(2026, 1, 1);
        reviewed.effective_until = NaiveDate::from_ymd_opt(2026, 12, 31);
        let mut include = FormSuggestion::active("2550Q", FormSuggestionSource::ReviewedCor);
        include.source_reference = Some("cor:page:1".into());
        let mut exclude = include.clone();
        exclude.active = false;
        exclude.source_reference = Some("cor:page:2".into());
        let mut set = reconcile_forms_set_for_year(2026, None, &[reviewed]).forms_set;
        set.entries.extend(
            reconcile_forms_set_for_year(2026, None, &[include, exclude])
                .forms_set
                .entries,
        );
        set.entries
            .sort_by(|left, right| left.form_code.cmp(&right.form_code));

        db.save_per_year_forms(tin, 2026, &set).unwrap();
        let loaded = db.get_per_year_forms(tin, 2026).unwrap();

        assert_eq!(loaded, set);
    }
}
