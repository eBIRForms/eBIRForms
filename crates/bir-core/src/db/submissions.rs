//! Submission repository — save and list form submissions.

use rusqlite::params;
use std::collections::BTreeMap;

use super::{Database, DbError, Submission};

/// One submission row without its `form_data` payload.
///
/// The agent control plane lists submissions on the GPUI UI thread. Decoding
/// every row's JSON blob there cost more than the rest of the frame put
/// together, so listings that only need identity read these columns instead.
#[derive(Debug, Clone)]
pub struct SubmissionSummary {
    pub id: i64,
    pub tin: String,
    pub form_type: String,
    pub period: String,
    pub status: String,
}

impl Database {
    /// Save a submission.
    pub fn save_submission(&self, mut sub: Submission) -> Result<Submission, DbError> {
        let json_data = serde_json::to_string(&sub.form_data)?;

        if let Some(id) = sub.id {
            self.conn.execute(
                "UPDATE submissions SET form_type = ?1, period = ?2, status = ?3, form_data = ?4, submitted_at = ?5, filename = ?6, updated_at = datetime('now') WHERE id = ?7",
                params![sub.form_type, sub.period, sub.status, json_data, sub.submitted_at, sub.filename, id],
            )?;
        } else {
            self.conn.execute(
                "INSERT INTO submissions (tin, form_type, period, status, form_data, submitted_at, filename) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![sub.tin, sub.form_type, sub.period, sub.status, json_data, sub.submitted_at, sub.filename],
            )?;
            sub.id = Some(self.conn.last_insert_rowid());
        }

        Ok(sub)
    }

    /// List submissions for a specific TIN.
    pub fn list_submissions_for_tin(&self, tin: &str) -> Result<Vec<Submission>, DbError> {
        let mut stmt = self.conn.prepare("SELECT id, tin, form_type, period, status, form_data, submitted_at, filename, created_at, updated_at FROM submissions WHERE tin = ?1 ORDER BY created_at DESC")?;
        let rows = stmt.query_map(params![tin], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, Option<String>>(9)?,
            ))
        })?;

        let mut submissions = Vec::new();
        for row_result in rows {
            let (
                id,
                tin,
                form_type,
                period,
                status,
                json_data,
                submitted_at,
                filename,
                created_at,
                updated_at,
            ) = row_result?;
            let form_data: BTreeMap<String, String> = serde_json::from_str(&json_data)?;

            submissions.push(Submission {
                id: Some(id),
                tin,
                form_type,
                period,
                status,
                form_data,
                submitted_at,
                filename,
                created_at,
                updated_at,
            });
        }
        Ok(submissions)
    }

    /// List submissions for a TIN without decoding each row's `form_data`.
    pub fn list_submission_summaries_for_tin(
        &self,
        tin: &str,
    ) -> Result<Vec<SubmissionSummary>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, tin, form_type, period, status FROM submissions WHERE tin = ?1 ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map(params![tin], |row| {
            Ok(SubmissionSummary {
                id: row.get(0)?,
                tin: row.get(1)?,
                form_type: row.get(2)?,
                period: row.get(3)?,
                status: row.get(4)?,
            })
        })?;
        let mut summaries = Vec::new();
        for row in rows {
            summaries.push(row?);
        }
        Ok(summaries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(tin: &str, period: &str) -> Submission {
        let mut form_data = BTreeMap::new();
        form_data.insert("tax_14".to_string(), "0.00".to_string());
        Submission {
            id: None,
            tin: tin.to_string(),
            form_type: "1601Cv2018".to_string(),
            period: period.to_string(),
            status: "Submitted".to_string(),
            form_data,
            submitted_at: None,
            filename: None,
            created_at: None,
            updated_at: None,
        }
    }

    fn profile(tin_segments: [&str; 4]) -> crate::profile::TaxpayerProfile {
        serde_json::from_value(serde_json::json!({
            "id": null,
            "full_name": "Test Taxpayer",
            "tin": {
                "segment1": tin_segments[0],
                "segment2": tin_segments[1],
                "segment3": tin_segments[2],
                "branch": tin_segments[3]
            },
            "rdo_code": "018",
            "line_of_business": "Retail",
            "registered_address": "Manila",
            "zip_code": "1000",
            "phone": "09123456789",
            "email": "test@example.com",
            "default_form_type": "1601Cv2018",
            "taxpayer_type": "Individual"
        }))
        .expect("test profile")
    }

    /// The lean listing must stay identity-equivalent to the full one. It is
    /// what the agent control plane reads on the UI thread.
    #[test]
    fn summaries_match_full_rows_without_form_data() {
        let db = Database::open_ephemeral().expect("ephemeral db");
        db.save_profile(profile(["123", "456", "789", "000"]))
            .expect("listed taxpayer");
        db.save_profile(profile(["999", "999", "999", "000"]))
            .expect("other taxpayer");
        db.save_submission(row("123456789000", "092026"))
            .expect("first row");
        db.save_submission(row("123456789000", "102026"))
            .expect("second row");
        db.save_submission(row("999999999000", "092026"))
            .expect("other taxpayer");

        let full = db
            .list_submissions_for_tin("123456789000")
            .expect("full rows");
        let summaries = db
            .list_submission_summaries_for_tin("123456789000")
            .expect("summaries");

        assert_eq!(summaries.len(), full.len());
        for (summary, row) in summaries.iter().zip(&full) {
            assert_eq!(Some(summary.id), row.id);
            assert_eq!(summary.tin, row.tin);
            assert_eq!(summary.form_type, row.form_type);
            assert_eq!(summary.period, row.period);
            assert_eq!(summary.status, row.status);
        }
        assert!(summaries.iter().all(|item| item.tin == "123456789000"));
    }
}
