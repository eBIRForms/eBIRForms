//! "Revert to Draft" (D7A + D8A) for 1601C and 2551Q, which keep their own
//! storage instead of [`crate::forms::queueable::QueueableForm`]. Same
//! semantics as [`Database::revert_queueable_for_amendment`]:
//!
//! - Queued (unclaimed): plain revert; nothing reached BIR. A claimed queue
//!   keeps the human-confirmed release path.
//! - Submitted or Confirmed: keep a read-only snapshot of what BIR received
//!   in `amendment_snapshots`, revert, set "Amended Return?" to Yes, and
//!   save the Draft under the same key.
//! - Paid or Draft: refused.
//!
//! Everything happens in one transaction; on any error nothing changes.

use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::amendment_snapshots::{NewAmendmentSnapshot, insert_amendment_snapshot};
use super::drafts::filing_status_to_db;
use super::{AmendmentBase, Database, DbError, RevertedForAmendment, SnapshotPayloadSource};
use crate::forms::form_1601c::Form1601CDraft;
use crate::forms::form_1601c_xml::FORM_1601C_FORM_ID;
use crate::forms::form_2551q::Form2551QDraft;
use crate::forms::form_2551q_xml::FORM_2551Q_FORM_ID;
use crate::forms::{FilingPeriod, FilingStatus};

/// What the shared revert needs from each form's own model.
trait LegacyAmendable: Clone + Serialize + DeserializeOwned {
    const FORM_CODE: &'static str;
    const LAYOUT_ID: &'static str;
    fn status(&self) -> &FilingStatus;
    fn updated_at(&self) -> &str;
    fn is_unclaimed(&self) -> bool;
    fn tin(&self) -> &str;
    fn taxable_year(&self) -> u16;
    fn filing_period(&self) -> FilingPeriod;
    fn submitted_at(&self) -> Option<&str>;
    fn confirmed_at(&self) -> Option<&str>;
    fn receipt_id(&self) -> Option<i64>;
    fn submission_filename(&self) -> Option<&str>;
    fn submitted_payload(&self) -> Option<&str>;
    fn field_map(&self) -> std::collections::BTreeMap<String, String>;
    fn revert_to_draft(&mut self);
    /// Set "Amended Return?" to Yes the way the form's editor does.
    fn mark_amended(&mut self);
    fn set_id(&mut self, id: Option<i64>);
}

impl LegacyAmendable for Form1601CDraft {
    const FORM_CODE: &'static str = "1601C";
    const LAYOUT_ID: &'static str = FORM_1601C_FORM_ID;
    fn status(&self) -> &FilingStatus {
        &self.status
    }
    fn updated_at(&self) -> &str {
        &self.updated_at
    }
    fn is_unclaimed(&self) -> bool {
        self.submission_claim_token.is_none() && self.submission_claimed_at.is_none()
    }
    fn tin(&self) -> &str {
        &self.tin
    }
    fn taxable_year(&self) -> u16 {
        self.taxable_year
    }
    fn filing_period(&self) -> FilingPeriod {
        FilingPeriod::Monthly(self.month)
    }
    fn submitted_at(&self) -> Option<&str> {
        self.submitted_at.as_deref()
    }
    fn confirmed_at(&self) -> Option<&str> {
        self.confirmed_at.as_deref()
    }
    fn receipt_id(&self) -> Option<i64> {
        self.receipt_id
    }
    fn submission_filename(&self) -> Option<&str> {
        self.submission_filename.as_deref()
    }
    fn submitted_payload(&self) -> Option<&str> {
        self.submitted_payload.as_deref()
    }
    fn field_map(&self) -> std::collections::BTreeMap<String, String> {
        self.to_bir_field_map()
    }
    fn revert_to_draft(&mut self) {
        Form1601CDraft::revert_to_draft(self);
    }
    fn mark_amended(&mut self) {
        Form1601CDraft::mark_amended(self);
    }
    fn set_id(&mut self, id: Option<i64>) {
        self.id = id;
    }
}

impl LegacyAmendable for Form2551QDraft {
    const FORM_CODE: &'static str = "2551Q";
    const LAYOUT_ID: &'static str = FORM_2551Q_FORM_ID;
    fn status(&self) -> &FilingStatus {
        &self.status
    }
    fn updated_at(&self) -> &str {
        &self.updated_at
    }
    fn is_unclaimed(&self) -> bool {
        self.submission_claim_token.is_none() && self.submission_claimed_at.is_none()
    }
    fn tin(&self) -> &str {
        &self.tin
    }
    fn taxable_year(&self) -> u16 {
        self.taxable_year
    }
    fn filing_period(&self) -> FilingPeriod {
        FilingPeriod::Quarterly(self.quarter)
    }
    fn submitted_at(&self) -> Option<&str> {
        self.submitted_at.as_deref()
    }
    fn confirmed_at(&self) -> Option<&str> {
        self.confirmed_at.as_deref()
    }
    fn receipt_id(&self) -> Option<i64> {
        self.receipt_id
    }
    fn submission_filename(&self) -> Option<&str> {
        self.submission_filename.as_deref()
    }
    fn submitted_payload(&self) -> Option<&str> {
        self.submitted_payload.as_deref()
    }
    fn field_map(&self) -> std::collections::BTreeMap<String, String> {
        self.to_bir_field_map()
    }
    fn revert_to_draft(&mut self) {
        Form2551QDraft::revert_to_draft(self);
    }
    fn mark_amended(&mut self) {
        Form2551QDraft::mark_amended(self);
    }
    fn set_id(&mut self, id: Option<i64>) {
        self.id = id;
    }
}

/// The stored plaintext, else a rebuild from the frozen field map with the
/// form's official writer, else nothing.
fn sent_payload<F: LegacyAmendable>(draft: &F) -> (Option<String>, SnapshotPayloadSource) {
    if let Some(payload) = draft.submitted_payload() {
        return (Some(payload.to_string()), SnapshotPayloadSource::Stored);
    }
    let rebuilt = crate::official_xml::layout(F::LAYOUT_ID)
        .ok()
        .and_then(|layout| crate::official_xml::write(layout, &draft.field_map()).ok());
    match rebuilt {
        Some(xml) => (Some(xml), SnapshotPayloadSource::Regenerated),
        None => (None, SnapshotPayloadSource::Unavailable),
    }
}

impl Database {
    /// The user's "Revert to Draft" for 1601C (D7A + D8A).
    pub fn revert_1601c_for_amendment(
        &self,
        tin: &str,
        taxable_year: u16,
        month: u8,
    ) -> Result<RevertedForAmendment<Form1601CDraft>, DbError> {
        self.revert_legacy_for_amendment(tin, taxable_year, i64::from(month), None, None)
    }

    /// An amendment's first save for 1601C in one transaction (see
    /// [`Database::save_queueable_amendment`]).
    pub fn save_1601c_amendment(
        &self,
        amended: &Form1601CDraft,
        base: &AmendmentBase,
    ) -> Result<i64, DbError> {
        let period = i64::from(amended.month);
        self.revert_legacy_for_amendment(
            amended.tin(),
            amended.taxable_year,
            period,
            Some(base),
            Some(amended),
        )?
        .draft
        .id
        .ok_or_else(|| DbError::Other("1601C amendment has no row".into()))
    }

    /// An amendment's first save for 2551Q in one transaction.
    pub fn save_2551q_amendment(
        &self,
        amended: &Form2551QDraft,
        base: &AmendmentBase,
    ) -> Result<i64, DbError> {
        let period = i64::from(amended.quarter);
        self.revert_legacy_for_amendment(
            amended.tin(),
            amended.taxable_year,
            period,
            Some(base),
            Some(amended),
        )?
        .draft
        .id
        .ok_or_else(|| DbError::Other("2551Q amendment has no row".into()))
    }

    /// The user's "Revert to Draft" for 2551Q (D7A + D8A).
    pub fn revert_2551q_for_amendment(
        &self,
        tin: &str,
        taxable_year: u16,
        quarter: u8,
    ) -> Result<RevertedForAmendment<Form2551QDraft>, DbError> {
        self.revert_legacy_for_amendment(tin, taxable_year, i64::from(quarter), None, None)
    }

    fn revert_legacy_for_amendment<F: LegacyAmendable>(
        &self,
        tin: &str,
        taxable_year: u16,
        period: i64,
        base: Option<&AmendmentBase>,
        replacement: Option<&F>,
    ) -> Result<RevertedForAmendment<F>, DbError> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let Some((id, raw_json, db_status)) = tx
            .query_row(
                "SELECT id, data_json, status FROM form_drafts
                 WHERE tin = ?1 AND form_code = ?2 AND taxable_year = ?3 AND quarter = ?4",
                params![tin, F::FORM_CODE, i64::from(taxable_year), period],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?
        else {
            return Err(DbError::Other(format!(
                "No {} return exists for this period",
                F::FORM_CODE
            )));
        };
        let current: F = serde_json::from_str(&raw_json)?;
        let previous_status = current.status().clone();
        if let Some(base) = base
            && (base.status != previous_status || base.updated_at != current.updated_at())
        {
            return Err(DbError::Other(format!(
                "The filed {} return changed since this amendment began; open it again and amend that",
                F::FORM_CODE
            )));
        }
        if db_status != filing_status_to_db(&previous_status) {
            return Err(DbError::Other(format!(
                "{} return status is inconsistent ({db_status} row, {previous_status:?} data); refusing to revert",
                F::FORM_CODE
            )));
        }
        if !crate::forms::queueable::can_user_revert(&previous_status) {
            return Err(DbError::Other(match previous_status {
                FilingStatus::Paid => {
                    format!("A Paid {} return cannot be reverted to Draft", F::FORM_CODE)
                }
                _ => format!("This {} return is already a Draft", F::FORM_CODE),
            }));
        }
        if !current.is_unclaimed() {
            return Err(DbError::Other(format!(
                "{} submission is in progress; wait for it to finish before reverting",
                F::FORM_CODE
            )));
        }

        let mut draft = current.clone();
        let mut snapshot_id = None;
        let mut marked_amended = false;
        if matches!(
            previous_status,
            FilingStatus::Sent | FilingStatus::Confirmed
        ) {
            let (xml, source) = sent_payload(&current);
            let period_key = current.filing_period().to_period_key();
            snapshot_id = Some(insert_amendment_snapshot(
                &tx,
                &NewAmendmentSnapshot {
                    form_code: F::FORM_CODE,
                    tin: current.tin(),
                    taxable_year: current.taxable_year(),
                    period_key: &period_key,
                    status_at_revert: filing_status_to_db(&previous_status),
                    submitted_at: current.submitted_at(),
                    confirmed_at: current.confirmed_at(),
                    receipt_id: current.receipt_id(),
                    submission_filename: current.submission_filename(),
                    submitted_xml: xml.as_deref(),
                    payload_source: source,
                    draft_json: &raw_json,
                },
            )?);
            draft.revert_to_draft();
            draft.mark_amended();
            marked_amended = true;
        } else {
            draft.revert_to_draft();
        }
        draft.set_id(Some(id));

        let json = serde_json::to_string(&draft)?;
        let updated = tx.execute(
            "UPDATE form_drafts SET status = 'Draft', data_json = ?1, updated_at = datetime('now')
             WHERE id = ?2 AND status = ?3 AND data_json = ?4",
            params![json, id, db_status, raw_json],
        )?;
        if updated != 1 {
            return Err(DbError::Other(format!(
                "{} return changed before the revert completed",
                F::FORM_CODE
            )));
        }
        // An amendment's first save, in the same transaction.
        if let Some(amended) = replacement {
            if !matches!(amended.status(), FilingStatus::Draft) {
                return Err(DbError::Other(format!(
                    "Only a Draft {} amendment may be saved",
                    F::FORM_CODE
                )));
            }
            let mut amended = amended.clone();
            amended.set_id(Some(id));
            tx.execute(
                "UPDATE form_drafts SET status = 'Draft', data_json = ?1, updated_at = datetime('now')
                 WHERE id = ?2",
                params![serde_json::to_string(&amended)?, id],
            )?;
            draft = amended;
        }
        tx.commit()?;
        let _ = self.request_post_commit_refresh();
        Ok(RevertedForAmendment {
            draft,
            previous_status,
            snapshot_id,
            marked_amended,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::TaxpayerProfile;

    const TIN: &str = "123456789000";

    fn profile() -> TaxpayerProfile {
        let mut profile: TaxpayerProfile = serde_json::from_value(serde_json::json!({
            "id": null,
            "full_name": "Test Taxpayer",
            "tin": {"segment1": "123", "segment2": "456", "segment3": "789", "branch": "000"},
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
        .unwrap();
        profile.ensure_profile_version_ledger();
        profile
    }

    fn queued_1601c() -> Form1601CDraft {
        let mut draft = Form1601CDraft::new_from_profile(&profile(), 2026, 5);
        draft.any_taxes_withheld = false;
        draft.compute();
        draft.transition_to_queued().expect("queue 1601C");
        draft
    }

    fn queued_2551q() -> Form2551QDraft {
        let mut draft = Form2551QDraft::new_from_effective_profile(&profile(), 2026, 1);
        draft.item_13_election = crate::forms::form_2551q::Item13Election::Graduated;
        draft.transition_to_queued().expect("queue 2551Q");
        draft
    }

    /// Advance a queued draft to `status` and store it as the period's row.
    fn store<F: LegacyAmendable>(
        db: &Database,
        draft: &F,
        status: &FilingStatus,
        period: i64,
    ) -> i64 {
        db.conn
            .execute(
                "INSERT INTO form_drafts
                    (tin, form_code, taxable_year, quarter, period_key, status, data_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    draft.tin(),
                    F::FORM_CODE,
                    i64::from(draft.taxable_year()),
                    period,
                    draft.filing_period().to_period_key(),
                    filing_status_to_db(status),
                    serde_json::to_string(draft).unwrap()
                ],
            )
            .unwrap();
        db.conn.last_insert_rowid()
    }

    fn row(db: &Database, form_code: &str) -> (String, String) {
        db.conn
            .query_row(
                "SELECT status, data_json FROM form_drafts WHERE form_code = ?1",
                params![form_code],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap()
    }

    fn snapshot_count(db: &Database) -> i64 {
        db.conn
            .query_row("SELECT COUNT(*) FROM amendment_snapshots", [], |row| {
                row.get(0)
            })
            .unwrap()
    }

    fn submitted_1601c(confirmed: bool) -> Form1601CDraft {
        let mut draft = queued_1601c();
        let filename = draft.default_submission_filename();
        draft.transition_to_submitted(filename);
        if confirmed {
            draft.transition_to_confirmed("2026-06-10T08:00:00".into(), Some(7), None);
        }
        draft
    }

    fn submitted_2551q(confirmed: bool) -> Form2551QDraft {
        let mut draft = queued_2551q();
        let filename = draft.default_submission_filename();
        draft.transition_to_submitted(filename);
        if confirmed {
            draft.transition_to_confirmed("2026-04-30T08:00:00".into(), Some(9), None);
        }
        draft
    }

    #[test]
    fn queue_stores_the_official_plaintext() {
        let q = queued_1601c();
        assert_eq!(
            q.submitted_payload.as_deref(),
            Some(q.to_bir_xml_payload().as_str())
        );
        let q = queued_2551q();
        assert_eq!(
            q.submitted_payload.as_deref(),
            Some(q.to_bir_xml_payload().unwrap().as_str())
        );
        let mut reverted = q;
        reverted.revert_to_draft();
        assert!(reverted.submitted_payload.is_none());
    }

    #[test]
    fn queued_1601c_reverts_plainly() {
        let db = Database::open_in_memory_for_tests().unwrap();
        let queued = queued_1601c();
        store(&db, &queued, &FilingStatus::Queued, 5);
        let out = db.revert_1601c_for_amendment(TIN, 2026, 5).unwrap();
        assert!(out.snapshot_id.is_none());
        assert!(!out.marked_amended);
        assert!(!out.draft.is_amended);
        assert!(matches!(out.draft.status, FilingStatus::Draft));
        assert_eq!(row(&db, "1601C").0, "Draft");
        assert_eq!(snapshot_count(&db), 0);
    }

    #[test]
    fn claimed_queue_is_refused_and_left_alone() {
        let db = Database::open_in_memory_for_tests().unwrap();
        let mut queued = queued_1601c();
        queued.submission_claim_token = Some("token".into());
        queued.submission_claimed_at = Some("2026-06-01T00:00:00+00:00".into());
        store(&db, &queued, &FilingStatus::Queued, 5);
        let before = row(&db, "1601C");
        assert!(db.revert_1601c_for_amendment(TIN, 2026, 5).is_err());
        assert_eq!(row(&db, "1601C"), before);
    }

    #[test]
    fn submitted_and_confirmed_1601c_keep_a_snapshot_and_become_amended() {
        for confirmed in [false, true] {
            let db = Database::open_in_memory_for_tests().unwrap();
            let sent = submitted_1601c(confirmed);
            let status = sent.status.clone();
            let raw = serde_json::to_string(&sent).unwrap();
            let id = store(&db, &sent, &status, 5);
            let out = db.revert_1601c_for_amendment(TIN, 2026, 5).unwrap();
            assert_eq!(out.previous_status, status);
            assert!(out.marked_amended);
            assert!(out.draft.is_amended);
            assert_eq!(out.draft.id, Some(id));
            assert!(out.draft.submitted_payload.is_none());
            let (db_status, json) = row(&db, "1601C");
            assert_eq!(db_status, "Draft");
            let stored: Form1601CDraft = serde_json::from_str(&json).unwrap();
            assert!(stored.is_amended);

            let snap = db
                .get_amendment_snapshot(out.snapshot_id.unwrap())
                .unwrap()
                .unwrap();
            assert_eq!(snap.form_code, "1601C");
            assert_eq!(snap.period_key, FilingPeriod::Monthly(5).to_period_key());
            assert_eq!(
                snap.status_at_revert,
                if confirmed { "Confirmed" } else { "Submitted" }
            );
            assert_eq!(snap.submitted_xml, sent.submitted_payload);
            assert_eq!(snap.payload_source, SnapshotPayloadSource::Stored);
            assert_eq!(snap.submission_filename, sent.submission_filename);
            assert_eq!(snap.receipt_id, if confirmed { Some(7) } else { None });
            assert_eq!(snap.draft_json, raw);
        }
    }

    #[test]
    fn submitted_and_confirmed_2551q_keep_a_snapshot_and_become_amended() {
        for confirmed in [false, true] {
            let db = Database::open_in_memory_for_tests().unwrap();
            let sent = submitted_2551q(confirmed);
            let status = sent.status.clone();
            store(&db, &sent, &status, 1);
            let out = db.revert_2551q_for_amendment(TIN, 2026, 1).unwrap();
            assert!(out.marked_amended);
            assert!(out.draft.is_amended);
            assert!(matches!(out.draft.status, FilingStatus::Draft));
            let snap = db
                .get_amendment_snapshot(out.snapshot_id.unwrap())
                .unwrap()
                .unwrap();
            assert_eq!(snap.form_code, "2551Q");
            assert_eq!(snap.period_key, "Q1");
            assert_eq!(snap.submitted_xml, sent.submitted_payload);
            assert_eq!(snap.payload_source, SnapshotPayloadSource::Stored);
            let stored: Form2551QDraft = serde_json::from_str(&row(&db, "2551Q").1).unwrap();
            assert!(stored.is_amended);
        }
    }

    #[test]
    fn queued_2551q_reverts_plainly() {
        let db = Database::open_in_memory_for_tests().unwrap();
        store(&db, &queued_2551q(), &FilingStatus::Queued, 1);
        let out = db.revert_2551q_for_amendment(TIN, 2026, 1).unwrap();
        assert!(out.snapshot_id.is_none());
        assert!(!out.draft.is_amended);
        assert_eq!(row(&db, "2551Q").0, "Draft");
    }

    #[test]
    fn older_returns_without_stored_plaintext_are_regenerated() {
        let db = Database::open_in_memory_for_tests().unwrap();
        let mut sent = submitted_1601c(true);
        sent.submitted_payload = None;
        store(&db, &sent, &FilingStatus::Confirmed, 5);
        let out = db.revert_1601c_for_amendment(TIN, 2026, 5).unwrap();
        let snap = db
            .get_amendment_snapshot(out.snapshot_id.unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(snap.payload_source, SnapshotPayloadSource::Regenerated);
        assert_eq!(snap.submitted_xml, Some(sent.to_bir_xml_payload()));

        let db = Database::open_in_memory_for_tests().unwrap();
        let mut sent = submitted_2551q(false);
        sent.submitted_payload = None;
        store(&db, &sent, &FilingStatus::Sent, 1);
        let out = db.revert_2551q_for_amendment(TIN, 2026, 1).unwrap();
        let snap = db
            .get_amendment_snapshot(out.snapshot_id.unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(snap.payload_source, SnapshotPayloadSource::Regenerated);
        assert_eq!(snap.submitted_xml, Some(sent.to_bir_xml_payload().unwrap()));
    }

    #[test]
    fn paid_draft_and_missing_returns_are_refused() {
        let db = Database::open_in_memory_for_tests().unwrap();
        assert!(db.revert_2551q_for_amendment(TIN, 2026, 1).is_err());
        let mut paid = submitted_2551q(true);
        paid.transition_to_paid();
        store(&db, &paid, &FilingStatus::Paid, 1);
        let before = row(&db, "2551Q");
        let error = db.revert_2551q_for_amendment(TIN, 2026, 1).unwrap_err();
        assert!(error.to_string().contains("Paid"));
        assert_eq!(row(&db, "2551Q"), before);
        assert_eq!(snapshot_count(&db), 0);

        let db = Database::open_in_memory_for_tests().unwrap();
        let mut draft = Form1601CDraft::new_from_profile(&profile(), 2026, 5);
        draft.compute();
        store(&db, &draft, &FilingStatus::Draft, 5);
        assert!(db.revert_1601c_for_amendment(TIN, 2026, 5).is_err());
    }

    #[test]
    fn a_failed_update_rolls_back_the_snapshot() {
        let db = Database::open_in_memory_for_tests().unwrap();
        let sent = submitted_1601c(true);
        store(&db, &sent, &FilingStatus::Confirmed, 5);
        // Make the final UPDATE fail after the snapshot insert.
        db.conn
            .execute_batch(
                "CREATE TRIGGER fail_revert BEFORE UPDATE ON form_drafts
                 BEGIN SELECT RAISE(ABORT, 'blocked'); END;",
            )
            .unwrap();
        let before = row(&db, "1601C");
        assert!(db.revert_1601c_for_amendment(TIN, 2026, 5).is_err());
        assert_eq!(row(&db, "1601C"), before);
        assert_eq!(snapshot_count(&db), 0);
    }

    #[test]
    fn inconsistent_row_status_is_refused() {
        let db = Database::open_in_memory_for_tests().unwrap();
        let sent = submitted_2551q(false);
        store(&db, &sent, &FilingStatus::Confirmed, 1);
        assert!(db.revert_2551q_for_amendment(TIN, 2026, 1).is_err());
        assert_eq!(snapshot_count(&db), 0);
    }
}
