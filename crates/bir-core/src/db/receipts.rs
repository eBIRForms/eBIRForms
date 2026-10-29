//! Receipt repository — save and query submission receipts.

use rusqlite::params;

use super::{Database, DbError, SubmissionReceipt, parse_2551q_period};
use crate::filing_queue::{
    is_audited_1601c_receipt_form_type, parse_1601c_period, receipt_belongs_to_this_generation,
};
use crate::forms::FilingStatus;
use crate::receipt::{BirReceiptConfirmation, split_bir_filename};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiptConfirmationOutcome {
    Confirmed,
    Ignored,
}

pub(super) fn is_audited_2551q_receipt_form_type(form_type: &str) -> bool {
    matches!(form_type, "2551Qv2018" | "2551Q")
}

impl Database {
    pub fn save_submission_receipt(
        &self,
        receipt: &BirReceiptConfirmation,
    ) -> Result<(SubmissionReceipt, bool), DbError> {
        self.save_submission_receipt_with_email_date(receipt, None)
    }

    /// `email_received_at` is the message's `Date` header as RFC 3339. A
    /// re-save without it keeps the stored value.
    pub fn save_submission_receipt_with_email_date(
        &self,
        receipt: &BirReceiptConfirmation,
        email_received_at: Option<&str>,
    ) -> Result<(SubmissionReceipt, bool), DbError> {
        let (tin, form_type, period) = split_bir_filename(&receipt.filename)
            .unwrap_or_else(|| ("".to_string(), "".to_string(), "".to_string()));

        let received_date_str = receipt.date_received.to_string();
        let received_time_str = receipt.time_received.format("%H:%M:%S").to_string();

        if let Some(existing) = self.get_submission_receipt_by_filename(&receipt.filename)?
            && existing.received_date == received_date_str
            && existing.received_time == received_time_str
        {
            // The exact receipt we already processed. Rows from before the
            // email date was kept still learn it here, since every poll
            // re-reads the last 30 days of BIR mail.
            if existing.email_received_at.is_none()
                && let Some(email_received_at) = email_received_at
            {
                self.conn.execute(
                    "UPDATE submission_receipts SET email_received_at = ?2 WHERE id = ?1",
                    params![existing.id, email_received_at],
                )?;
                let healed = self
                    .get_submission_receipt_by_filename(&receipt.filename)?
                    .unwrap_or(existing);
                return Ok((healed, false));
            }
            return Ok((existing, false));
        }

        self.conn.execute(
            "INSERT INTO submission_receipts
                (filename, tin, form_type, period, received_date, received_time, source_from, raw_text, raw_html, email_received_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(filename) DO UPDATE SET
                tin = excluded.tin,
                form_type = excluded.form_type,
                period = excluded.period,
                received_date = excluded.received_date,
                received_time = excluded.received_time,
                source_from = excluded.source_from,
                raw_text = excluded.raw_text,
                raw_html = excluded.raw_html,
                email_received_at = COALESCE(excluded.email_received_at, submission_receipts.email_received_at)",
            params![
                receipt.filename,
                tin,
                form_type,
                period,
                receipt.date_received.to_string(),
                receipt.time_received.format("%H:%M:%S").to_string(),
                receipt.source_from,
                receipt.raw_text,
                receipt.raw_html,
                email_received_at,
            ],
        )?;

        let saved = self
            .get_submission_receipt_by_filename(&receipt.filename)?
            .expect("receipt should exist after save");
        Ok((saved, true))
    }

    pub fn get_submission_receipt_by_id(
        &self,
        id: i64,
    ) -> Result<Option<SubmissionReceipt>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, filename, tin, form_type, period, received_date, received_time,
                    source_from, raw_text, raw_html, created_at, email_received_at
             FROM submission_receipts WHERE id = ?1",
        )?;
        let mut rows = stmt.query(params![id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(SubmissionReceipt {
                id: row.get(0)?,
                filename: row.get(1)?,
                tin: row.get(2)?,
                form_type: row.get(3)?,
                period: row.get(4)?,
                received_date: row.get(5)?,
                received_time: row.get(6)?,
                source_from: row.get(7)?,
                raw_text: row.get(8)?,
                raw_html: row.get(9)?,
                email_received_at: row.get(11)?,
                created_at: row.get(10)?,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn get_submission_receipt_by_filename(
        &self,
        filename: &str,
    ) -> Result<Option<SubmissionReceipt>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, filename, tin, form_type, period, received_date, received_time,
                    source_from, raw_text, raw_html, created_at, email_received_at
             FROM submission_receipts WHERE filename = ?1",
        )?;
        let mut rows = stmt.query(params![filename])?;
        if let Some(row) = rows.next()? {
            Ok(Some(SubmissionReceipt {
                id: Some(row.get(0)?),
                filename: row.get(1)?,
                tin: row.get(2)?,
                form_type: row.get(3)?,
                period: row.get(4)?,
                received_date: row.get(5)?,
                received_time: row.get(6)?,
                source_from: row.get(7)?,
                raw_text: row.get(8)?,
                raw_html: row.get(9)?,
                email_received_at: row.get(11)?,
                created_at: row.get(10)?,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn confirm_2551q_from_receipt(
        &self,
        receipt: &SubmissionReceipt,
    ) -> Result<ReceiptConfirmationOutcome, DbError> {
        if !is_audited_2551q_receipt_form_type(&receipt.form_type) {
            return Ok(ReceiptConfirmationOutcome::Ignored);
        }

        let Some((year, quarter)) = parse_2551q_period(&receipt.period) else {
            return Ok(ReceiptConfirmationOutcome::Ignored);
        };

        let mut draft = match self.get_2551q_draft(&receipt.tin, year, quarter)? {
            Some(draft) => draft,
            None => return Ok(ReceiptConfirmationOutcome::Ignored),
        };

        // Only confirm forms that are currently in Submitted status
        if !matches!(draft.status, FilingStatus::Submitted) {
            return Ok(ReceiptConfirmationOutcome::Ignored);
        }
        let receipt_id = receipt.id.ok_or_else(|| {
            DbError::Other(
                "A 2551Q receipt must be persisted before it can confirm a submission".to_string(),
            )
        })?;

        let submitted_at = draft.submitted_at.as_deref().ok_or_else(|| {
            DbError::Other("Submitted 2551Q draft has no submission timestamp".to_string())
        })?;
        chrono::DateTime::parse_from_rfc3339(submitted_at).map_err(|error| {
            DbError::Other(format!(
                "Submitted 2551Q draft has an invalid submission timestamp: {error}"
            ))
        })?;
        let date_str = format!("{}T{}", receipt.received_date, receipt.received_time);
        let receipt_naive = chrono::NaiveDateTime::parse_from_str(&date_str, "%Y-%m-%dT%H:%M:%S")
            .map_err(|error| {
            DbError::Other(format!(
                "Receipt has an invalid received timestamp: {error}"
            ))
        })?;
        let offset = chrono::FixedOffset::east_opt(8 * 3600)
            .ok_or_else(|| DbError::Other("UTC+08:00 offset is unavailable".to_string()))?;
        use chrono::TimeZone;
        let receipt_dt = offset
            .from_local_datetime(&receipt_naive)
            .single()
            .ok_or_else(|| DbError::Other("Receipt received timestamp is ambiguous".to_string()))?;
        // BIR stamps the file when the SFTP PUT lands (minute precision);
        // `submitted_at` is written after PUT returns. Floor on the queue
        // time, as 1601C does, or a same-minute receipt is ignored forever.
        let authorized_at = draft
            .queue_authorization
            .as_ref()
            .map(|auth| auth.authorized_at.as_str());
        if !receipt_belongs_to_this_generation(receipt_dt, authorized_at, submitted_at) {
            tracing::info!(
                "Ignoring old receipt {} for 2551Q draft submitted at {}",
                receipt.filename,
                submitted_at
            );
            return Ok(ReceiptConfirmationOutcome::Ignored);
        }

        draft.transition_to_confirmed(date_str, Some(receipt_id), Some(receipt.filename.clone()));
        self.save_confirmed_2551q_draft(&draft)?;
        Ok(ReceiptConfirmationOutcome::Confirmed)
    }

    pub fn confirm_1601c_from_receipt(
        &self,
        receipt: &SubmissionReceipt,
    ) -> Result<ReceiptConfirmationOutcome, DbError> {
        if !is_audited_1601c_receipt_form_type(&receipt.form_type) {
            return Ok(ReceiptConfirmationOutcome::Ignored);
        }

        let Some((year, month)) = parse_1601c_period(&receipt.period) else {
            return Ok(ReceiptConfirmationOutcome::Ignored);
        };

        let mut draft = match self.get_1601c_draft(&receipt.tin, year, month)? {
            Some(draft) => draft,
            None => return Ok(ReceiptConfirmationOutcome::Ignored),
        };

        if !matches!(draft.status, FilingStatus::Submitted) {
            return Ok(ReceiptConfirmationOutcome::Ignored);
        }
        let receipt_id = receipt.id.ok_or_else(|| {
            DbError::Other(
                "A 1601C receipt must be persisted before it can confirm a submission".to_string(),
            )
        })?;

        let submitted_at = draft.submitted_at.as_deref().ok_or_else(|| {
            DbError::Other("Submitted 1601C draft has no submission timestamp".to_string())
        })?;
        chrono::DateTime::parse_from_rfc3339(submitted_at).map_err(|error| {
            DbError::Other(format!(
                "Submitted 1601C draft has an invalid submission timestamp: {error}"
            ))
        })?;
        let date_str = format!("{}T{}", receipt.received_date, receipt.received_time);
        let receipt_naive = chrono::NaiveDateTime::parse_from_str(&date_str, "%Y-%m-%dT%H:%M:%S")
            .map_err(|error| {
            DbError::Other(format!(
                "Receipt has an invalid received timestamp: {error}"
            ))
        })?;
        let offset = chrono::FixedOffset::east_opt(8 * 3600)
            .ok_or_else(|| DbError::Other("UTC+08:00 offset is unavailable".to_string()))?;
        use chrono::TimeZone;
        let receipt_dt = offset
            .from_local_datetime(&receipt_naive)
            .single()
            .ok_or_else(|| DbError::Other("Receipt received timestamp is ambiguous".to_string()))?;
        let authorized_at = draft
            .queue_authorization
            .as_ref()
            .map(|auth| auth.authorized_at.as_str());
        if !receipt_belongs_to_this_generation(receipt_dt, authorized_at, submitted_at) {
            tracing::info!(
                "Ignoring old receipt {} for 1601C draft submitted at {}",
                receipt.filename,
                submitted_at
            );
            return Ok(ReceiptConfirmationOutcome::Ignored);
        }

        draft.transition_to_confirmed(date_str, Some(receipt_id), Some(receipt.filename.clone()));
        self.save_confirmed_1601c_draft(&draft)?;
        Ok(ReceiptConfirmationOutcome::Confirmed)
    }

    /// Confirm whichever queued form the receipt belongs to.
    pub fn confirm_submission_from_receipt(
        &self,
        receipt: &SubmissionReceipt,
    ) -> Result<ReceiptConfirmationOutcome, DbError> {
        if is_audited_1601c_receipt_form_type(&receipt.form_type) {
            self.confirm_1601c_from_receipt(receipt)
        } else if is_audited_2551q_receipt_form_type(&receipt.form_type) {
            self.confirm_2551q_from_receipt(receipt)
        } else if let Some(kind) =
            crate::forms::queueable::QueueableKind::from_form_type(&receipt.form_type)
        {
            crate::with_queueable_kind!(kind, F => self.confirm_queueable_from_receipt::<F>(receipt))
        } else {
            Ok(ReceiptConfirmationOutcome::Ignored)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forms::form_1601c::Form1601CDraft;

    fn receipt(form_type: &str) -> SubmissionReceipt {
        SubmissionReceipt {
            id: None,
            filename: format!("123456789000-{form_type}-122026Q1.xml"),
            tin: "123456789000".to_string(),
            form_type: form_type.to_string(),
            period: "122026Q1".to_string(),
            received_date: "2026-04-25".to_string(),
            received_time: "12:00:00".to_string(),
            source_from: None,
            raw_text: "test receipt".to_string(),
            raw_html: None,
            email_received_at: None,
            created_at: None,
        }
    }

    #[test]
    fn email_date_is_stored_and_kept_across_a_re_save_without_it() {
        let db = Database::open_in_memory_for_tests().unwrap();
        let confirmation = BirReceiptConfirmation {
            filename: "123456789000-1601Cv2018-092026.xml".to_string(),
            date_received: chrono::NaiveDate::from_ymd_opt(2026, 9, 12).unwrap(),
            time_received: chrono::NaiveTime::from_hms_opt(14, 52, 0).unwrap(),
            source_from: Some("ebirforms-noreply@bir.gov.ph".to_string()),
            raw_text: "This confirms receipt".to_string(),
            raw_html: None,
        };
        let (saved, is_new) = db
            .save_submission_receipt_with_email_date(
                &confirmation,
                Some("2026-09-12T14:59:00+08:00"),
            )
            .unwrap();
        assert!(is_new);
        assert_eq!(
            saved.email_received_at.as_deref(),
            Some("2026-09-12T14:59:00+08:00")
        );

        let again = db
            .get_submission_receipt_by_id(saved.id.unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(
            again.email_received_at.as_deref(),
            Some("2026-09-12T14:59:00+08:00")
        );
    }

    #[test]
    fn a_receipt_stored_before_the_email_date_learns_it_on_the_next_poll() {
        let db = Database::open_in_memory_for_tests().unwrap();
        let confirmation = BirReceiptConfirmation {
            filename: "123456789000-1601Cv2018-112026.xml".to_string(),
            date_received: chrono::NaiveDate::from_ymd_opt(2026, 9, 12).unwrap(),
            time_received: chrono::NaiveTime::from_hms_opt(14, 52, 0).unwrap(),
            source_from: None,
            raw_text: "This confirms receipt".to_string(),
            raw_html: None,
        };
        let (first, _) = db.save_submission_receipt(&confirmation).unwrap();
        assert!(first.email_received_at.is_none());

        let (healed, is_new) = db
            .save_submission_receipt_with_email_date(
                &confirmation,
                Some("2026-09-12T14:59:00+08:00"),
            )
            .unwrap();
        assert!(!is_new, "same receipt, not a new one");
        assert_eq!(healed.id, first.id);
        assert_eq!(
            healed.email_received_at.as_deref(),
            Some("2026-09-12T14:59:00+08:00")
        );
    }

    #[test]
    fn receipt_form_aliases_are_closed_and_explicit() {
        assert!(is_audited_2551q_receipt_form_type("2551Qv2018"));
        assert!(is_audited_2551q_receipt_form_type("2551Q"));
        assert!(!is_audited_2551q_receipt_form_type("2551Qv2024"));
        assert!(!is_audited_2551q_receipt_form_type("1601C"));
    }

    #[test]
    fn unrelated_or_unmatched_receipts_report_ignored() {
        let database = Database::open_in_memory_for_tests().unwrap();
        assert_eq!(
            database
                .confirm_2551q_from_receipt(&receipt("1601C"))
                .unwrap(),
            ReceiptConfirmationOutcome::Ignored
        );
        assert_eq!(
            database
                .confirm_2551q_from_receipt(&receipt("2551Q"))
                .unwrap(),
            ReceiptConfirmationOutcome::Ignored
        );
    }

    #[test]
    fn save_submission_receipt_strips_iaf_email_from_period() {
        let database = Database::open_in_memory_for_tests().unwrap();
        let confirmation = BirReceiptConfirmation {
            filename: "00000000000000-1601Cv2018-092026#taxpayer@example.com#.xml".to_string(),
            date_received: chrono::NaiveDate::from_ymd_opt(2026, 9, 10).unwrap(),
            time_received: chrono::NaiveTime::from_hms_opt(14, 30, 0).unwrap(),
            source_from: Some("ebirforms-noreply@bir.gov.ph".to_string()),
            raw_text: "test".to_string(),
            raw_html: None,
        };
        let (saved, is_new) = database.save_submission_receipt(&confirmation).unwrap();
        assert!(is_new);
        assert_eq!(saved.tin, "00000000000000");
        assert_eq!(saved.form_type, "1601Cv2018");
        assert_eq!(saved.period, "092026");
        assert_eq!(
            saved.filename,
            "00000000000000-1601Cv2018-092026#taxpayer@example.com#.xml"
        );
    }

    #[test]
    fn confirm_1601c_from_receipt_advances_submitted_fixture() {
        use crate::db::Claim1601CSubmissionResult;
        use crate::filing_queue::mandatory_lab_1601c_draft;
        use crate::forms::FilingStatus;

        let db = Database::open_in_memory_for_tests().unwrap();
        let mut queued = mandatory_lab_1601c_draft();
        queued.transition_to_queued().unwrap();
        db.save_queued_1601c_draft(&queued).unwrap();
        let fingerprint = queued.queued_submission_fingerprint.clone();
        let retry = queued.next_retry_at.clone();
        let claim = db
            .claim_queued_1601c_submission(
                &queued.tin,
                queued.taxable_year,
                queued.month,
                &fingerprint,
                &retry,
                0,
            )
            .unwrap();
        let Claim1601CSubmissionResult::Claimed { mut draft, token } = claim else {
            panic!("expected claim");
        };
        let filename = queued.default_submission_filename();
        draft.transition_to_submitted(filename.clone());
        db.finish_claimed_1601c_submission(&draft, &token).unwrap();

        let confirmation = BirReceiptConfirmation {
            filename: filename.clone(),
            date_received: chrono::NaiveDate::from_ymd_opt(2099, 12, 31).unwrap(),
            time_received: chrono::NaiveTime::from_hms_opt(15, 0, 0).unwrap(),
            source_from: Some("ebirforms-noreply@bir.gov.ph".to_string()),
            raw_text: "test".to_string(),
            raw_html: None,
        };
        let (saved, _) = db.save_submission_receipt(&confirmation).unwrap();
        assert_eq!(
            db.confirm_submission_from_receipt(&saved).unwrap(),
            ReceiptConfirmationOutcome::Confirmed
        );
        let confirmed = db
            .get_1601c_draft(&queued.tin, queued.taxable_year, queued.month)
            .unwrap()
            .unwrap();
        assert_eq!(confirmed.status, FilingStatus::Confirmed);
        assert_eq!(confirmed.tax_36_total_amount_payable, 0.0);
        assert_eq!(
            confirmed.submission_filename.as_deref(),
            Some(filename.as_str())
        );
    }

    fn submit_lab_1601c(db: &Database, mut queued: Form1601CDraft) -> Form1601CDraft {
        use crate::db::Claim1601CSubmissionResult;

        queued.transition_to_queued().unwrap();
        db.save_queued_1601c_draft(&queued).unwrap();
        let fingerprint = queued.queued_submission_fingerprint.clone();
        let retry = queued.next_retry_at.clone();
        let claim = db
            .claim_queued_1601c_submission(
                &queued.tin,
                queued.taxable_year,
                queued.month,
                &fingerprint,
                &retry,
                0,
            )
            .unwrap();
        let Claim1601CSubmissionResult::Claimed { mut draft, token } = claim else {
            panic!("expected claim");
        };
        let filename = queued.default_submission_filename();
        draft.transition_to_submitted(filename);
        db.finish_claimed_1601c_submission(&draft, &token).unwrap();
        db.get_1601c_draft(&queued.tin, queued.taxable_year, queued.month)
            .unwrap()
            .unwrap()
    }

    fn overwrite_1601c_generation_timestamps(
        db: &Database,
        draft: &Form1601CDraft,
        authorized_at: &str,
        submitted_at: &str,
    ) {
        let mut stored = db
            .get_1601c_draft(&draft.tin, draft.taxable_year, draft.month)
            .unwrap()
            .unwrap();
        stored
            .queue_authorization
            .as_mut()
            .expect("queued fixture records authorization")
            .authorized_at = authorized_at.to_string();
        stored.submitted_at = Some(submitted_at.to_string());
        let json = serde_json::to_string(&stored).unwrap();
        db.conn
            .execute(
                "UPDATE form_drafts SET data_json = ?1
                 WHERE tin = ?2 AND form_code = '1601C'
                   AND taxable_year = ?3 AND quarter = ?4",
                rusqlite::params![
                    json,
                    stored.tin,
                    i64::from(stored.taxable_year),
                    i64::from(stored.month)
                ],
            )
            .unwrap();
    }

    #[test]
    fn confirm_1601c_matches_stripped_october_receipt_filename() {
        use crate::filing_queue::mandatory_lab_1601c_october_2026_draft;
        use crate::forms::FilingStatus;

        let db = Database::open_in_memory_for_tests().unwrap();
        let submitted = submit_lab_1601c(&db, mandatory_lab_1601c_october_2026_draft());
        let iaf = submitted
            .submission_filename
            .clone()
            .expect("submitted IAF");
        assert_eq!(
            iaf,
            "00000000000000-1601Cv2018-102026#taxpayer@example.com#.xml"
        );

        // BIR echoes the filename without the `#email#` part.
        let confirmation = BirReceiptConfirmation {
            filename: "00000000000000-1601Cv2018-102026.xml".to_string(),
            date_received: chrono::NaiveDate::from_ymd_opt(2099, 12, 31).unwrap(),
            time_received: chrono::NaiveTime::from_hms_opt(15, 13, 0).unwrap(),
            source_from: Some("ebirforms-noreply@bir.gov.ph".to_string()),
            raw_text: "This confirms receipt".to_string(),
            raw_html: None,
        };
        let (saved, _) = db.save_submission_receipt(&confirmation).unwrap();
        assert_eq!(saved.period, "102026");
        assert_eq!(
            db.confirm_submission_from_receipt(&saved).unwrap(),
            ReceiptConfirmationOutcome::Confirmed
        );
        let confirmed = db
            .get_1601c_draft(&submitted.tin, submitted.taxable_year, submitted.month)
            .unwrap()
            .unwrap();
        assert_eq!(confirmed.status, FilingStatus::Confirmed);
        assert_eq!(confirmed.month, 10);
        assert_eq!(confirmed.tax_36_total_amount_payable, 0.0);
        assert_eq!(confirmed.submission_filename.as_deref(), Some(iaf.as_str()));
    }

    #[test]
    fn confirm_1601c_accepts_live_put_to_wal_delay() {
        use crate::filing_queue::mandatory_lab_1601c_october_2026_draft;
        use crate::forms::FilingStatus;

        let db = Database::open_in_memory_for_tests().unwrap();
        let submitted = submit_lab_1601c(&db, mandatory_lab_1601c_october_2026_draft());
        overwrite_1601c_generation_timestamps(
            &db,
            &submitted,
            "2026-09-11T15:09:54+08:00",
            "2026-09-11T15:17:25+08:00",
        );

        let confirmation = BirReceiptConfirmation {
            filename: "00000000000000-1601Cv2018-102026.xml".to_string(),
            date_received: chrono::NaiveDate::from_ymd_opt(2026, 9, 11).unwrap(),
            time_received: chrono::NaiveTime::from_hms_opt(15, 13, 0).unwrap(),
            source_from: Some("ebirforms-noreply@bir.gov.ph".to_string()),
            raw_text: "live 102026 receipt".to_string(),
            raw_html: None,
        };
        let (saved, _) = db.save_submission_receipt(&confirmation).unwrap();
        assert_eq!(
            db.confirm_submission_from_receipt(&saved).unwrap(),
            ReceiptConfirmationOutcome::Confirmed
        );
        let confirmed = db
            .get_1601c_draft(&submitted.tin, submitted.taxable_year, submitted.month)
            .unwrap()
            .unwrap();
        assert_eq!(confirmed.status, FilingStatus::Confirmed);
    }

    #[test]
    fn confirm_1601c_retries_after_saved_receipt_was_ignored() {
        use crate::filing_queue::mandatory_lab_1601c_october_2026_draft;
        use crate::forms::FilingStatus;

        let db = Database::open_in_memory_for_tests().unwrap();
        let submitted = submit_lab_1601c(&db, mandatory_lab_1601c_october_2026_draft());
        overwrite_1601c_generation_timestamps(
            &db,
            &submitted,
            "2026-09-12T15:09:54+08:00",
            "2026-09-12T15:17:25+08:00",
        );

        let confirmation = BirReceiptConfirmation {
            filename: "00000000000000-1601Cv2018-102026.xml".to_string(),
            date_received: chrono::NaiveDate::from_ymd_opt(2026, 9, 11).unwrap(),
            time_received: chrono::NaiveTime::from_hms_opt(15, 13, 0).unwrap(),
            source_from: Some("ebirforms-noreply@bir.gov.ph".to_string()),
            raw_text: "prior-day receipt".to_string(),
            raw_html: None,
        };
        let (saved, _) = db.save_submission_receipt(&confirmation).unwrap();
        assert_eq!(
            db.confirm_submission_from_receipt(&saved).unwrap(),
            ReceiptConfirmationOutcome::Ignored
        );

        overwrite_1601c_generation_timestamps(
            &db,
            &submitted,
            "2026-09-11T15:09:54+08:00",
            "2026-09-11T15:17:25+08:00",
        );
        assert_eq!(
            db.confirm_submission_from_receipt(&saved).unwrap(),
            ReceiptConfirmationOutcome::Confirmed
        );
        let confirmed = db
            .get_1601c_draft(&submitted.tin, submitted.taxable_year, submitted.month)
            .unwrap()
            .unwrap();
        assert_eq!(confirmed.status, FilingStatus::Confirmed);
    }
}
