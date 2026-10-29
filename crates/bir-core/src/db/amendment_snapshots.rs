//! `amendment_snapshots`: read-only copies of returns BIR already has.
//!
//! When a Submitted or Confirmed return is reverted to Draft for an
//! amendment (D8A), the version BIR received is kept here first. Rows are
//! only ever inserted; nothing in the app updates or deletes them.

use rusqlite::{OptionalExtension, Transaction, params};

use super::{Database, DbError};

/// Where [`AmendmentSnapshot::submitted_xml`] came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotPayloadSource {
    /// The plaintext stored with the return when it was queued and submitted.
    Stored,
    /// A return queued before plaintext was stored: rebuilt from its frozen,
    /// fingerprinted field map with the current official writer.
    Regenerated,
    /// Neither was possible; only the metadata and draft JSON are kept.
    Unavailable,
}

impl SnapshotPayloadSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stored => "stored",
            Self::Regenerated => "regenerated",
            Self::Unavailable => "unavailable",
        }
    }

    fn parse(value: &str) -> Self {
        match value {
            "stored" => Self::Stored,
            "regenerated" => Self::Regenerated,
            _ => Self::Unavailable,
        }
    }
}

/// One kept copy of a return as BIR received it.
#[derive(Debug, Clone, PartialEq)]
pub struct AmendmentSnapshot {
    pub id: i64,
    pub form_code: String,
    pub tin: String,
    pub taxable_year: u16,
    pub period_key: String,
    /// `Submitted` or `Confirmed`.
    pub status_at_revert: String,
    pub submitted_at: Option<String>,
    pub confirmed_at: Option<String>,
    pub receipt_id: Option<i64>,
    pub submission_filename: Option<String>,
    /// Plaintext official XML (before compression and encryption).
    pub submitted_xml: Option<String>,
    pub payload_source: SnapshotPayloadSource,
    /// The full stored draft JSON at revert time.
    pub draft_json: String,
    pub created_at: String,
}

/// Fields of a snapshot to insert.
#[derive(Debug, Clone)]
pub(crate) struct NewAmendmentSnapshot<'a> {
    pub form_code: &'a str,
    pub tin: &'a str,
    pub taxable_year: u16,
    pub period_key: &'a str,
    pub status_at_revert: &'a str,
    pub submitted_at: Option<&'a str>,
    pub confirmed_at: Option<&'a str>,
    pub receipt_id: Option<i64>,
    pub submission_filename: Option<&'a str>,
    pub submitted_xml: Option<&'a str>,
    pub payload_source: SnapshotPayloadSource,
    pub draft_json: &'a str,
}

const COLUMNS: &str = "id, form_code, tin, taxable_year, period_key, status_at_revert,
    submitted_at, confirmed_at, receipt_id, submission_filename, submitted_xml,
    payload_source, draft_json, created_at";

fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AmendmentSnapshot> {
    let year: i64 = row.get(3)?;
    let source: String = row.get(11)?;
    Ok(AmendmentSnapshot {
        id: row.get(0)?,
        form_code: row.get(1)?,
        tin: row.get(2)?,
        taxable_year: u16::try_from(year).unwrap_or_default(),
        period_key: row.get(4)?,
        status_at_revert: row.get(5)?,
        submitted_at: row.get(6)?,
        confirmed_at: row.get(7)?,
        receipt_id: row.get(8)?,
        submission_filename: row.get(9)?,
        submitted_xml: row.get(10)?,
        payload_source: SnapshotPayloadSource::parse(&source),
        draft_json: row.get(12)?,
        created_at: row.get(13)?,
    })
}

/// Insert inside the caller's transaction so it commits with the revert.
pub(crate) fn insert_amendment_snapshot(
    tx: &Transaction<'_>,
    snapshot: &NewAmendmentSnapshot<'_>,
) -> Result<i64, DbError> {
    tx.execute(
        "INSERT INTO amendment_snapshots
            (form_code, tin, taxable_year, period_key, status_at_revert, submitted_at,
             confirmed_at, receipt_id, submission_filename, submitted_xml, payload_source,
             draft_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            snapshot.form_code,
            snapshot.tin,
            i64::from(snapshot.taxable_year),
            snapshot.period_key,
            snapshot.status_at_revert,
            snapshot.submitted_at,
            snapshot.confirmed_at,
            snapshot.receipt_id,
            snapshot.submission_filename,
            snapshot.submitted_xml,
            snapshot.payload_source.as_str(),
            snapshot.draft_json,
        ],
    )?;
    Ok(tx.last_insert_rowid())
}

impl Database {
    /// Snapshots of one return (form + TIN + year + period key), oldest first.
    pub fn list_amendment_snapshots(
        &self,
        form_code: &str,
        tin: &str,
        taxable_year: u16,
        period_key: &str,
    ) -> Result<Vec<AmendmentSnapshot>, DbError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM amendment_snapshots
             WHERE form_code = ?1 AND tin = ?2 AND taxable_year = ?3 AND period_key = ?4
             ORDER BY id"
        ))?;
        let rows = stmt.query_map(
            params![form_code, tin, i64::from(taxable_year), period_key],
            from_row,
        )?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Every snapshot for a TIN, newest first.
    pub fn list_amendment_snapshots_for_tin(
        &self,
        tin: &str,
    ) -> Result<Vec<AmendmentSnapshot>, DbError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM amendment_snapshots WHERE tin = ?1 ORDER BY id DESC"
        ))?;
        let rows = stmt.query_map(params![tin], from_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// save-009's "an original already exists": another return for this TIN,
    /// form, year and period that is Submitted, Confirmed or Paid, or any
    /// amendment snapshot of it. A Draft or Queued return is not an original,
    /// and the row `exclude_draft_id` (the record being saved) never counts.
    pub fn has_filed_original(
        &self,
        tin: &str,
        form_code: &str,
        taxable_year: u16,
        period_key: &str,
        exclude_draft_id: Option<i64>,
    ) -> Result<bool, DbError> {
        let year = i64::from(taxable_year);
        let filed: bool = self.conn.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM form_drafts
                 WHERE tin = ?1 AND form_code = ?2 AND taxable_year = ?3
                   AND period_key = ?4
                   AND status IN ('Submitted', 'Confirmed', 'Paid')
                   AND (?5 IS NULL OR id <> ?5))",
            params![tin, form_code, year, period_key, exclude_draft_id],
            |row| row.get(0),
        )?;
        if filed {
            return Ok(true);
        }
        Ok(self.conn.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM amendment_snapshots
                 WHERE tin = ?1 AND form_code = ?2 AND taxable_year = ?3 AND period_key = ?4)",
            params![tin, form_code, year, period_key],
            |row| row.get(0),
        )?)
    }

    pub fn get_amendment_snapshot(&self, id: i64) -> Result<Option<AmendmentSnapshot>, DbError> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM amendment_snapshots WHERE id = ?1"),
                params![id],
                from_row,
            )
            .optional()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::TransactionBehavior;

    fn insert(db: &Database, period_key: &str, xml: Option<&str>) -> i64 {
        let tx = Transaction::new_unchecked(&db.conn, TransactionBehavior::Immediate).unwrap();
        let id = insert_amendment_snapshot(
            &tx,
            &NewAmendmentSnapshot {
                form_code: "2550Q",
                tin: "123-456-788-00000",
                taxable_year: 2025,
                period_key,
                status_at_revert: "Confirmed",
                submitted_at: Some("2025-07-01T00:00:00+00:00"),
                confirmed_at: Some("2025-07-01T08:00:00"),
                receipt_id: Some(7),
                submission_filename: Some("file.xml"),
                submitted_xml: xml,
                payload_source: if xml.is_some() {
                    SnapshotPayloadSource::Stored
                } else {
                    SnapshotPayloadSource::Unavailable
                },
                draft_json: "{}",
            },
        )
        .unwrap();
        tx.commit().unwrap();
        id
    }

    #[test]
    fn snapshots_insert_list_and_get() {
        let db = Database::open_in_memory_for_tests().unwrap();
        let first = insert(&db, "Q2", Some("<xml/>"));
        let second = insert(&db, "Q2", None);
        insert(&db, "Q3", None);

        let listed = db
            .list_amendment_snapshots("2550Q", "123-456-788-00000", 2025, "Q2")
            .unwrap();
        assert_eq!(
            listed.iter().map(|s| s.id).collect::<Vec<_>>(),
            vec![first, second]
        );
        let got = db.get_amendment_snapshot(first).unwrap().unwrap();
        assert_eq!(got.submitted_xml.as_deref(), Some("<xml/>"));
        assert_eq!(got.payload_source, SnapshotPayloadSource::Stored);
        assert_eq!(got.receipt_id, Some(7));
        assert_eq!(got.status_at_revert, "Confirmed");
        assert_eq!(
            db.get_amendment_snapshot(second)
                .unwrap()
                .unwrap()
                .payload_source,
            SnapshotPayloadSource::Unavailable
        );
        assert_eq!(
            db.list_amendment_snapshots_for_tin("123-456-788-00000")
                .unwrap()
                .len(),
            3
        );
        assert!(db.get_amendment_snapshot(9999).unwrap().is_none());
    }
}
