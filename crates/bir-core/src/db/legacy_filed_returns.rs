use rusqlite::{Connection, params};

use super::{Database, DbError};

/// Canonical `<TIN9><branch5>` form used by `legacy_filed_returns.tin`:
/// 9-digit root plus the 3-digit profile branch zero-padded to five digits
/// (`123-456-789-002` -> `12345678900002`). Unknown shapes pass through.
pub(crate) fn canonical_legacy_tin(tin: &str) -> String {
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

#[derive(Debug, Clone)]
pub struct LegacyFiledReturn {
    pub id: Option<i64>,
    pub sha256: String,
    pub tin: String,
    pub form_identity: String,
    pub form_id: Option<String>,
    pub period: String,
    pub amendment_version: Option<u32>,
    pub email_copy: Option<String>,
    pub encrypted: bool,
    pub source_filename: String,
    pub raw_file: Vec<u8>,
    pub fields_json: String,
    pub imported_at: Option<String>,
}

impl Database {
    pub fn insert_legacy_filed_return(&self, record: &LegacyFiledReturn) -> Result<bool, DbError> {
        Self::insert_legacy_filed_return_on(&self.conn, record)
    }

    pub(crate) fn insert_legacy_filed_return_on(
        connection: &Connection,
        record: &LegacyFiledReturn,
    ) -> Result<bool, DbError> {
        let inserted = connection.execute(
            "INSERT OR IGNORE INTO legacy_filed_returns (
                sha256, tin, form_identity, form_id, period, amendment_version,
                email_copy, encrypted, source_filename, raw_file, fields_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                record.sha256,
                record.tin,
                record.form_identity,
                record.form_id,
                record.period,
                record.amendment_version.map(i64::from),
                record.email_copy,
                record.encrypted,
                record.source_filename,
                record.raw_file,
                record.fields_json,
            ],
        )?;
        Ok(inserted == 1)
    }

    pub fn list_legacy_filed_returns_for_tin(
        &self,
        tin: &str,
    ) -> Result<Vec<LegacyFiledReturn>, DbError> {
        let canonical_tin = canonical_legacy_tin(tin);
        let mut statement = self.conn.prepare(
            "SELECT id, sha256, tin, form_identity, form_id, period,
                    amendment_version, email_copy, encrypted, source_filename,
                    raw_file, fields_json, imported_at
             FROM legacy_filed_returns
             WHERE tin = ?1
             ORDER BY imported_at, id",
        )?;
        let rows = statement.query_map(params![canonical_tin], |row| {
            let amendment_version = row
                .get::<_, Option<i64>>(6)?
                .map(u32::try_from)
                .transpose()
                .map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        6,
                        rusqlite::types::Type::Integer,
                        Box::new(error),
                    )
                })?;
            Ok(LegacyFiledReturn {
                id: Some(row.get(0)?),
                sha256: row.get(1)?,
                tin: row.get(2)?,
                form_identity: row.get(3)?,
                form_id: row.get(4)?,
                period: row.get(5)?,
                amendment_version,
                email_copy: row.get(7)?,
                encrypted: row.get(8)?,
                source_filename: row.get(9)?,
                raw_file: row.get(10)?,
                fields_json: row.get(11)?,
                imported_at: row.get(12)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(DbError::from)
    }
}
