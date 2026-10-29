//! `form_drafts` storage for every [`QueueableForm`].
//!
//! Each function mirrors its 1601C counterpart in `drafts.rs`: the same
//! status rules and the same compare-and-swap on `data_json`, so two workers
//! or a worker and the user can never both win a transition.

use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};

use super::amendment_snapshots::{
    NewAmendmentSnapshot, SnapshotPayloadSource, insert_amendment_snapshot,
};
use super::drafts::{
    AbandonedClaimRelease, abandoned_claim_audit, claim_fields_partial, claim_fields_present,
    filing_status_to_db, require_abandoned_release_reason,
};
use super::receipts::ReceiptConfirmationOutcome;
use super::{Database, DbError, SubmissionReceipt};
use crate::forms::FilingStatus;
use crate::forms::queueable::QueueableForm;

/// Result of claiming one exact queued generation before PUT.
pub enum ClaimQueueableResult<F> {
    Claimed {
        draft: F,
        token: String,
    },
    Rejected {
        draft: F,
        errors: Vec<(String, String)>,
    },
    Superseded,
}

/// The filed return an amendment was copied from, as it was then: its first
/// save goes ahead only while the stored row is still exactly that.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmendmentBase {
    pub status: FilingStatus,
    pub updated_at: String,
}

/// Result of [`Database::revert_queueable_for_amendment`].
#[derive(Debug, Clone)]
pub struct RevertedForAmendment<F> {
    /// The editable Draft now stored under the same key.
    pub draft: F,
    pub previous_status: FilingStatus,
    /// `amendment_snapshots.id` when the return had reached BIR.
    pub snapshot_id: Option<i64>,
    /// True when the form's "Amended Return?" item was set to Yes.
    pub marked_amended: bool,
}

/// The plaintext BIR received for `draft`: the stored bytes when present,
/// else a rebuild from its frozen field map (returns queued before the
/// plaintext was stored), else nothing.
fn sent_payload<F: QueueableForm>(draft: &F) -> (Option<String>, SnapshotPayloadSource) {
    if let Some(payload) = &draft.lifecycle().submitted_payload {
        return (Some(payload.clone()), SnapshotPayloadSource::Stored);
    }
    let rebuilt = crate::official_xml::layout(F::LAYOUT_ID)
        .ok()
        .and_then(|layout| crate::official_xml::write(layout, &draft.field_map()).ok());
    match rebuilt {
        Some(xml) => (Some(xml), SnapshotPayloadSource::Regenerated),
        None => (None, SnapshotPayloadSource::Unavailable),
    }
}

fn errors_summary(errors: &[(String, String)]) -> String {
    errors
        .iter()
        .map(|(field, message)| format!("{field}: {message}"))
        .collect::<Vec<_>>()
        .join("; ")
}

type Row = (i64, String, String);

fn select_row(
    tx: &Transaction<'_>,
    form_code: &str,
    tin: &str,
    taxable_year: u16,
    period: i64,
) -> Result<Option<Row>, DbError> {
    Ok(tx
        .query_row(
            "SELECT id, data_json, status FROM form_drafts
             WHERE tin = ?1 AND form_code = ?2 AND taxable_year = ?3 AND quarter = ?4",
            params![tin, form_code, i64::from(taxable_year), period],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?)
}

/// Annual and open-ended forms saved by their pre-generic editors stored a
/// NULL `quarter` (the period lives in `period_key`). Give such a row this
/// form's period column so every generic lookup finds it and it is updated
/// in place instead of duplicated.
fn adopt_legacy_null_period_row(
    tx: &Transaction<'_>,
    form_code: &str,
    tin: &str,
    taxable_year: u16,
    period: i64,
    period_key: &str,
) -> Result<(), DbError> {
    tx.execute(
        "UPDATE form_drafts SET quarter = ?4
         WHERE id = (
             SELECT id FROM form_drafts
             WHERE tin = ?1 AND form_code = ?2 AND taxable_year = ?3
               AND quarter IS NULL AND period_key = ?5
             ORDER BY updated_at DESC, id DESC
             LIMIT 1
         )
         AND NOT EXISTS (
             SELECT 1 FROM form_drafts
             WHERE tin = ?1 AND form_code = ?2 AND taxable_year = ?3 AND quarter = ?4
         )",
        params![tin, form_code, i64::from(taxable_year), period, period_key],
    )?;
    Ok(())
}

/// Refusal when a re-keyed save would land on another return's Draft.
pub const REKEY_COLLISION_MESSAGE: &str =
    "Another draft already uses this date; open it from the library";

/// When `draft` was loaded or saved as row `draft.draft_id()` and its period
/// key has since changed (e.g. the 2200 Item 1 day or a transaction date),
/// move that row to the new key so exactly one Draft remains.
///
/// Returns `Ok(Some(id))` when the row was moved, `Ok(None)` when the normal
/// upsert should proceed. A non-Draft previous row is never touched, and an
/// existing Draft at the new key that belongs to another row is refused.
fn move_rekeyed_draft<F: QueueableForm>(
    tx: &Transaction<'_>,
    draft: &F,
    target: Option<&Row>,
    period: i64,
    period_key: &str,
    status: &str,
) -> Result<Option<i64>, DbError> {
    let Some(previous_id) = draft.draft_id() else {
        return Ok(None);
    };
    if let Some((target_id, _, target_status)) = target {
        if *target_id == previous_id {
            return Ok(None);
        }
        if target_status == "Draft" {
            return Err(DbError::Other(REKEY_COLLISION_MESSAGE.to_string()));
        }
        // A queued or filed row at the new key: the normal path refuses it.
        return Ok(None);
    }
    let previous: Option<(String, String, String)> = tx
        .query_row(
            "SELECT tin, status, data_json FROM form_drafts WHERE id = ?1 AND form_code = ?2",
            params![previous_id, F::FORM_CODE],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let Some((previous_tin, previous_status, previous_json)) = previous else {
        return Ok(None);
    };
    if previous_tin != draft.tin() || previous_status != "Draft" {
        return Ok(None);
    }
    let stored: F = serde_json::from_str(&previous_json)?;
    if !stored.lifecycle().is_editable() {
        return Ok(None);
    }
    let mut moved = draft.clone();
    moved.set_draft_id(Some(previous_id));
    let updated = tx.execute(
        "UPDATE form_drafts
         SET taxable_year = ?1, quarter = ?2, period_key = ?3, status = ?4,
             data_json = ?5, updated_at = datetime('now')
         WHERE id = ?6 AND status = 'Draft' AND data_json = ?7",
        params![
            i64::from(draft.taxable_year()),
            period,
            period_key,
            status,
            serde_json::to_string(&moved)?,
            previous_id,
            previous_json
        ],
    )?;
    if updated != 1 {
        return Err(DbError::Other(format!(
            "{} draft changed before the save completed",
            F::FORM_CODE
        )));
    }
    Ok(Some(previous_id))
}

impl Database {
    pub fn get_queueable_draft<F: QueueableForm>(
        &self,
        tin: &str,
        taxable_year: u16,
        period: i64,
    ) -> Result<Option<F>, DbError> {
        let raw: Option<(i64, String)> = self
            .conn
            .query_row(
                "SELECT id, data_json FROM form_drafts
                 WHERE tin = ?1 AND form_code = ?2 AND taxable_year = ?3 AND quarter = ?4",
                params![tin, F::FORM_CODE, i64::from(taxable_year), period],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        raw.map(|(id, json)| {
            let mut draft: F = serde_json::from_str(&json)?;
            draft.set_draft_id(Some(id));
            Ok(draft)
        })
        .transpose()
    }

    /// Every stored `F` return of a TIN and year, each with its period
    /// column (an entry-numbered form's entry, a transaction's `MMDD`), in
    /// one read.
    pub fn list_queueable_drafts<F: QueueableForm>(
        &self,
        tin: &str,
        taxable_year: u16,
    ) -> Result<Vec<(i64, F)>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, quarter, data_json FROM form_drafts
             WHERE tin = ?1 AND form_code = ?2 AND taxable_year = ?3
             ORDER BY quarter",
        )?;
        let rows = stmt.query_map(params![tin, F::FORM_CODE, i64::from(taxable_year)], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let mut drafts = Vec::new();
        for row in rows {
            let (id, period, json) = row?;
            let mut draft: F = serde_json::from_str(&json)?;
            draft.set_draft_id(Some(id));
            drafts.push((period.unwrap_or(0), draft));
        }
        Ok(drafts)
    }

    /// Save an editable Draft. A queued or filed snapshot is never replaced.
    pub fn save_queueable_draft<F: QueueableForm>(&self, draft: &F) -> Result<i64, DbError> {
        if !draft.lifecycle().is_editable() {
            return Err(DbError::Other(format!(
                "Only an editable Draft {} return may use the draft save path",
                F::FORM_CODE
            )));
        }
        self.upsert_from_draft(draft, "Draft")
    }

    /// save-009: true when the form runs the official guard, its Amended
    /// Return is No, and a filed original of the same return exists.
    pub fn amended_return_required<F: QueueableForm>(&self, draft: &F) -> Result<bool, DbError> {
        if draft.amended_return_answer() != Some(false) {
            return Ok(false);
        }
        self.has_filed_original(
            draft.tin(),
            F::FORM_CODE,
            draft.taxable_year(),
            &draft.filing_period().to_period_key(),
            draft.draft_id(),
        )
    }

    /// Persist the exact reviewed queue snapshot after revalidating it.
    pub fn save_queued_queueable<F: QueueableForm>(&self, draft: &F) -> Result<i64, DbError> {
        if !matches!(draft.lifecycle().status, FilingStatus::Queued) {
            return Err(DbError::Other(format!(
                "Only a queued {} return can be saved through the submission path",
                F::FORM_CODE
            )));
        }
        if self.amended_return_required(draft)? {
            return Err(DbError::AmendedReturnRequired);
        }
        let mut verified = draft.clone();
        verified
            .revalidate_queued_before_submission()
            .map_err(|errors| {
                DbError::Other(format!(
                    "Queued {} return failed queue revalidation: {}",
                    F::FORM_CODE,
                    errors_summary(&errors)
                ))
            })?;
        self.upsert_from_draft(&verified, "Queued")
    }

    /// Write `draft` with `status` over an existing Draft row, or insert it.
    fn upsert_from_draft<F: QueueableForm>(&self, draft: &F, status: &str) -> Result<i64, DbError> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let json = serde_json::to_string(draft)?;
        let period = draft.period_column();
        let period_key = draft.filing_period().to_period_key();
        adopt_legacy_null_period_row(
            &tx,
            F::FORM_CODE,
            draft.tin(),
            draft.taxable_year(),
            period,
            &period_key,
        )?;
        let target = select_row(&tx, F::FORM_CODE, draft.tin(), draft.taxable_year(), period)?;
        if let Some(moved) =
            move_rekeyed_draft(&tx, draft, target.as_ref(), period, &period_key, status)?
        {
            tx.commit()?;
            let _ = self.request_post_commit_refresh();
            return Ok(moved);
        }
        let id = match target {
            Some((id, raw_json, db_status)) => {
                let stored: F = serde_json::from_str(&raw_json)?;
                if db_status != "Draft" || !stored.lifecycle().is_editable() {
                    return Err(DbError::Other(format!(
                        "A queued or filed {} snapshot must be canceled back to Draft before it can be replaced",
                        F::FORM_CODE
                    )));
                }
                let updated = tx.execute(
                    "UPDATE form_drafts
                     SET status = ?1, data_json = ?2, period_key = ?3, updated_at = datetime('now')
                     WHERE id = ?4 AND status = 'Draft' AND data_json = ?5",
                    params![status, json, period_key, id, raw_json],
                )?;
                if updated != 1 {
                    return Err(DbError::Other(format!(
                        "{} draft changed before the save completed",
                        F::FORM_CODE
                    )));
                }
                id
            }
            None => {
                tx.execute(
                    "INSERT INTO form_drafts
                        (tin, form_code, taxable_year, quarter, period_key, status, data_json)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        draft.tin(),
                        F::FORM_CODE,
                        i64::from(draft.taxable_year()),
                        period,
                        period_key,
                        status,
                        json
                    ],
                )?;
                tx.last_insert_rowid()
            }
        };
        // Record the row id inside the stored JSON so a later load and a
        // re-keyed save can name the row it came from.
        if draft.draft_id() != Some(id) {
            let mut stamped = draft.clone();
            stamped.set_draft_id(Some(id));
            tx.execute(
                "UPDATE form_drafts SET data_json = ?1 WHERE id = ?2",
                params![serde_json::to_string(&stamped)?, id],
            )?;
        }
        tx.commit()?;
        let _ = self.request_post_commit_refresh();
        Ok(id)
    }

    /// CAS-replace an unclaimed queue generation with a retry update or a
    /// return to Draft. The caller names the exact generation it loaded.
    pub(crate) fn replace_unclaimed_queued_queueable<F: QueueableForm>(
        &self,
        replacement: &F,
        expected_fingerprint: &Option<String>,
        expected_next_retry_at: &Option<String>,
        expected_submission_attempts: u32,
    ) -> Result<bool, DbError> {
        let mut replacement = replacement.clone();
        let status = replacement.lifecycle().status.clone();
        if !matches!(status, FilingStatus::Draft | FilingStatus::Queued) {
            return Err(DbError::Other(format!(
                "An unclaimed {} queue generation may only remain Queued or return to Draft",
                F::FORM_CODE
            )));
        }
        if matches!(status, FilingStatus::Queued) {
            if &replacement.lifecycle().queued_submission_fingerprint != expected_fingerprint {
                return Err(DbError::Other(format!(
                    "A retry update cannot change the reviewed {} queue fingerprint",
                    F::FORM_CODE
                )));
            }
            replacement
                .revalidate_queued_before_submission()
                .map_err(|errors| {
                    DbError::Other(format!(
                        "A retry update cannot persist invalid {} submission fields: {}",
                        F::FORM_CODE,
                        errors_summary(&errors)
                    ))
                })?;
        }
        if !replacement.lifecycle().is_unclaimed() {
            return Err(DbError::Other(format!(
                "An unclaimed {} replacement cannot carry a network claim",
                F::FORM_CODE
            )));
        }

        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let Some((id, raw_json, db_status)) = select_row(
            &tx,
            F::FORM_CODE,
            replacement.tin(),
            replacement.taxable_year(),
            replacement.period_column(),
        )?
        else {
            return Ok(false);
        };
        let current: F = serde_json::from_str(&raw_json)?;
        let lifecycle = current.lifecycle();
        if db_status != "Queued"
            || !matches!(lifecycle.status, FilingStatus::Queued)
            || !lifecycle.is_unclaimed()
            || &lifecycle.queued_submission_fingerprint != expected_fingerprint
            || &lifecycle.next_retry_at != expected_next_retry_at
            || lifecycle.submission_attempts != expected_submission_attempts
        {
            return Ok(false);
        }
        let json = serde_json::to_string(&replacement)?;
        let updated = tx.execute(
            "UPDATE form_drafts SET status = ?1, data_json = ?2, updated_at = datetime('now')
             WHERE id = ?3 AND status = 'Queued' AND data_json = ?4",
            params![
                filing_status_to_db(&replacement.lifecycle().status),
                json,
                id,
                raw_json
            ],
        )?;
        if updated != 1 {
            return Ok(false);
        }
        tx.commit()?;
        let _ = self.request_post_commit_refresh();
        Ok(true)
    }

    /// Cancel one exact, still-unclaimed queue generation back to Draft.
    pub fn cancel_queued_queueable<F: QueueableForm>(&self, queued: &F) -> Result<F, DbError> {
        if !matches!(queued.lifecycle().status, FilingStatus::Queued)
            || !queued.lifecycle().is_unclaimed()
        {
            return Err(DbError::Other(format!(
                "Only an unclaimed queued {} snapshot can be canceled",
                F::FORM_CODE
            )));
        }
        let mut verified = queued.clone();
        verified
            .revalidate_queued_before_submission()
            .map_err(|errors| {
                DbError::Other(format!(
                    "Only the exact reviewed {} queue snapshot can be canceled: {}",
                    F::FORM_CODE,
                    errors_summary(&errors)
                ))
            })?;
        let lifecycle = verified.lifecycle().clone();
        let mut draft = verified;
        draft.lifecycle_mut().revert_to_draft();
        if !self.replace_unclaimed_queued_queueable(
            &draft,
            &lifecycle.queued_submission_fingerprint,
            &lifecycle.next_retry_at,
            lifecycle.submission_attempts,
        )? {
            return Err(DbError::Other(format!(
                "{} submission has already started or the queue generation changed",
                F::FORM_CODE
            )));
        }
        Ok(draft)
    }

    /// The user's "Revert to Draft" (D7A + D8A). One editable record per
    /// period remains.
    ///
    /// - Queued (unclaimed): plain revert; nothing reached BIR.
    /// - Submitted or Confirmed: keep a read-only snapshot of what BIR
    ///   received, revert, set "Amended Return?" to Yes, and save the Draft
    ///   under the same key.
    /// - Paid or Draft: refused.
    ///
    /// Everything happens in one transaction; on any error nothing changes.
    pub fn revert_queueable_for_amendment<F: QueueableForm>(
        &self,
        tin: &str,
        taxable_year: u16,
        period: i64,
    ) -> Result<RevertedForAmendment<F>, DbError> {
        self.revert_queueable_inner(tin, taxable_year, period, None, None)
    }

    /// An amendment's first save, in ONE transaction: the user's revert of
    /// the filed return (kept byte for byte as its amendment snapshot) and
    /// `amended` written in its row. Refused, with nothing changed, when the
    /// row is no longer the filed return `base` describes.
    pub fn save_queueable_amendment<F: QueueableForm>(
        &self,
        amended: &F,
        base: &AmendmentBase,
    ) -> Result<i64, DbError> {
        let reverted = self.revert_queueable_inner(
            amended.tin(),
            amended.taxable_year(),
            amended.period_column(),
            Some(base),
            Some(amended),
        )?;
        reverted
            .draft
            .draft_id()
            .ok_or_else(|| DbError::Other(format!("{} amendment has no row", F::FORM_CODE)))
    }

    fn revert_queueable_inner<F: QueueableForm>(
        &self,
        tin: &str,
        taxable_year: u16,
        period: i64,
        base: Option<&AmendmentBase>,
        replacement: Option<&F>,
    ) -> Result<RevertedForAmendment<F>, DbError> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let Some((id, raw_json, db_status)) =
            select_row(&tx, F::FORM_CODE, tin, taxable_year, period)?
        else {
            return Err(DbError::Other(format!(
                "No {} return exists for this period",
                F::FORM_CODE
            )));
        };
        let current: F = serde_json::from_str(&raw_json)?;
        let previous_status = current.lifecycle().status.clone();
        if let Some(base) = base
            && (base.status != previous_status || base.updated_at != current.lifecycle().updated_at)
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
        if !current.lifecycle().is_unclaimed() {
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
            let lifecycle = current.lifecycle();
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
                    submitted_at: lifecycle.submitted_at.as_deref(),
                    confirmed_at: lifecycle.confirmed_at.as_deref(),
                    receipt_id: lifecycle.receipt_id,
                    submission_filename: lifecycle.submission_filename.as_deref(),
                    submitted_xml: xml.as_deref(),
                    payload_source: source,
                    draft_json: &raw_json,
                },
            )?);
            draft.lifecycle_mut().revert_to_draft();
            marked_amended = draft.mark_amended();
        } else {
            draft.lifecycle_mut().revert_to_draft();
        }
        draft.set_draft_id(Some(id));

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
        // An amendment's first save writes it over the reverted Draft in the
        // same transaction: a failure leaves the filed return as it was.
        if let Some(amended) = replacement {
            if !amended.lifecycle().is_editable() {
                return Err(DbError::Other(format!(
                    "Only a Draft {} amendment may be saved",
                    F::FORM_CODE
                )));
            }
            let mut amended = amended.clone();
            amended.set_draft_id(Some(id));
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

    /// Human-confirmed release of a claimed queue snapshot back to Draft.
    pub fn release_abandoned_claimed_queueable<F: QueueableForm>(
        &self,
        tin: &str,
        taxable_year: u16,
        period: i64,
        reason: &str,
    ) -> Result<AbandonedClaimRelease<F>, DbError> {
        require_abandoned_release_reason(reason)?;
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let Some((id, raw_json, db_status)) =
            select_row(&tx, F::FORM_CODE, tin, taxable_year, period)?
        else {
            return Ok(AbandonedClaimRelease::AlreadyClear {
                previous_status: None,
                draft: None,
                reason: reason.to_string(),
            });
        };
        let current: F = serde_json::from_str(&raw_json)?;
        let lifecycle = current.lifecycle();
        let claimed = claim_fields_present(
            &lifecycle.submission_claim_token,
            &lifecycle.submission_claimed_at,
        );
        let partial = claim_fields_partial(
            &lifecycle.submission_claim_token,
            &lifecycle.submission_claimed_at,
        );
        if matches!(lifecycle.status, FilingStatus::Draft)
            && db_status == "Draft"
            && !claimed
            && !partial
        {
            return Ok(AbandonedClaimRelease::AlreadyClear {
                previous_status: Some(FilingStatus::Draft),
                draft: Some(current),
                reason: reason.to_string(),
            });
        }
        if matches!(
            lifecycle.status,
            FilingStatus::Sent | FilingStatus::Confirmed | FilingStatus::Paid
        ) || matches!(db_status.as_str(), "Submitted" | "Confirmed" | "Paid")
        {
            return Err(DbError::Other(format!(
                "Will not release a {} claim that is already Submitted, Confirmed, or Paid",
                F::FORM_CODE
            )));
        }
        if partial {
            return Err(DbError::Other(format!(
                "{} claim metadata is incomplete; refusing to wipe an unknown filing state",
                F::FORM_CODE
            )));
        }
        if !(matches!(lifecycle.status, FilingStatus::Queued) && db_status == "Queued" && claimed) {
            return Err(DbError::Other(format!(
                "{} is not an abandoned claimed queue snapshot; unclaimed queues are canceled instead",
                F::FORM_CODE
            )));
        }
        let previous_claimed_at = lifecycle.submission_claimed_at.clone();
        let mut draft = current;
        draft.lifecycle_mut().revert_to_draft();
        draft.lifecycle_mut().submission_error = Some(abandoned_claim_audit(reason));
        let json = serde_json::to_string(&draft)?;
        let updated = tx.execute(
            "UPDATE form_drafts SET status = 'Draft', data_json = ?1, updated_at = datetime('now')
             WHERE id = ?2 AND status = 'Queued' AND data_json = ?3",
            params![json, id, raw_json],
        )?;
        if updated != 1 {
            return Err(DbError::Other(format!(
                "{} claim changed before abandoned-claim release completed",
                F::FORM_CODE
            )));
        }
        tx.commit()?;
        let _ = self.request_post_commit_refresh();
        Ok(AbandonedClaimRelease::Released {
            previous_status: FilingStatus::Queued,
            previous_claim_present: true,
            previous_claimed_at,
            draft,
            reason: reason.to_string(),
        })
    }

    /// Atomically revalidate and claim the exact queued generation right
    /// before PUT. The worker opens the upload session first, so a connect or
    /// login failure stays unclaimed.
    pub fn claim_queued_queueable<F: QueueableForm>(
        &self,
        tin: &str,
        taxable_year: u16,
        period: i64,
        expected_fingerprint: &Option<String>,
        expected_next_retry_at: &Option<String>,
        expected_submission_attempts: u32,
    ) -> Result<ClaimQueueableResult<F>, DbError> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let Some((id, raw_json, db_status)) =
            select_row(&tx, F::FORM_CODE, tin, taxable_year, period)?
        else {
            return Ok(ClaimQueueableResult::Superseded);
        };
        let mut draft: F = serde_json::from_str(&raw_json)?;
        let lifecycle = draft.lifecycle();
        if db_status != "Queued"
            || !matches!(lifecycle.status, FilingStatus::Queued)
            || !lifecycle.is_unclaimed()
            || &lifecycle.queued_submission_fingerprint != expected_fingerprint
            || &lifecycle.next_retry_at != expected_next_retry_at
            || lifecycle.submission_attempts != expected_submission_attempts
        {
            return Ok(ClaimQueueableResult::Superseded);
        }

        if let Err(errors) = draft.revalidate_queued_before_submission() {
            let rejected_json = serde_json::to_string(&draft)?;
            let updated = tx.execute(
                "UPDATE form_drafts SET status = 'Draft', data_json = ?1, updated_at = datetime('now')
                 WHERE id = ?2 AND status = 'Queued' AND data_json = ?3",
                params![rejected_json, id, raw_json],
            )?;
            if updated != 1 {
                return Ok(ClaimQueueableResult::Superseded);
            }
            tx.commit()?;
            return Ok(ClaimQueueableResult::Rejected { draft, errors });
        }

        let token = uuid::Uuid::new_v4().to_string();
        draft
            .lifecycle_mut()
            .claim(token.clone(), chrono::Utc::now());
        let claimed_json = serde_json::to_string(&draft)?;
        let updated = tx.execute(
            "UPDATE form_drafts SET data_json = ?1, updated_at = datetime('now')
             WHERE id = ?2 AND status = 'Queued' AND data_json = ?3",
            params![claimed_json, id, raw_json],
        )?;
        if updated != 1 {
            return Ok(ClaimQueueableResult::Superseded);
        }
        tx.commit()?;
        Ok(ClaimQueueableResult::Claimed { draft, token })
    }

    /// Record that PUT is about to start for the claim holder.
    pub fn mark_claimed_queueable_put_started<F: QueueableForm>(
        &self,
        tin: &str,
        taxable_year: u16,
        period: i64,
        claim_token: &str,
    ) -> Result<bool, DbError> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let Some((id, raw_json, db_status)) =
            select_row(&tx, F::FORM_CODE, tin, taxable_year, period)?
        else {
            return Ok(false);
        };
        if db_status != "Queued" {
            return Ok(false);
        }
        let mut draft: F = serde_json::from_str(&raw_json)?;
        if draft.lifecycle().submission_claim_token.as_deref() != Some(claim_token) {
            return Ok(false);
        }
        if draft.lifecycle().submission_put_started_at.is_none() {
            draft.lifecycle_mut().submission_put_started_at = Some(chrono::Utc::now().to_rfc3339());
        }
        let json = serde_json::to_string(&draft)?;
        let updated = tx.execute(
            "UPDATE form_drafts SET data_json = ?1, updated_at = datetime('now')
             WHERE id = ?2 AND status = 'Queued' AND data_json = ?3",
            params![json, id, raw_json],
        )?;
        if updated != 1 {
            return Ok(false);
        }
        tx.commit()?;
        Ok(true)
    }

    /// Recover a claim whose lease expired before PUT started.
    pub fn recover_expired_unstarted_queueable_claim<F: QueueableForm>(
        &self,
        tin: &str,
        taxable_year: u16,
        period: i64,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool, DbError> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let Some((id, raw_json, db_status)) =
            select_row(&tx, F::FORM_CODE, tin, taxable_year, period)?
        else {
            return Ok(false);
        };
        if db_status != "Queued" {
            return Ok(false);
        }
        let mut draft: F = serde_json::from_str(&raw_json)?;
        let lifecycle = draft.lifecycle();
        if !crate::filing_queue::claim_is_recoverable(
            lifecycle.submission_claimed_at.as_deref(),
            lifecycle.submission_claim_lease_until.as_deref(),
            lifecycle.submission_put_started_at.as_deref(),
            now,
        ) {
            return Ok(false);
        }
        if !draft.lifecycle_mut().release_expired_unstarted_claim(
            "Claim lease expired before PUT started; retrying the same authorized task".to_string(),
        ) {
            return Ok(false);
        }
        let json = serde_json::to_string(&draft)?;
        let updated = tx.execute(
            "UPDATE form_drafts SET status = 'Queued', data_json = ?1, updated_at = datetime('now')
             WHERE id = ?2 AND status = 'Queued' AND data_json = ?3",
            params![json, id, raw_json],
        )?;
        if updated != 1 {
            return Ok(false);
        }
        tx.commit()?;
        Ok(true)
    }

    /// The far side said nothing reached BIR: release this worker's claim
    /// and schedule a retry with backoff (five failures go back to Draft).
    pub(crate) fn fail_claimed_queueable_not_sent<F: QueueableForm>(
        &self,
        tin: &str,
        taxable_year: u16,
        period: i64,
        claim_token: &str,
        error: String,
    ) -> Result<bool, DbError> {
        self.update_claimed_queueable::<F>(tin, taxable_year, period, claim_token, |lifecycle| {
            lifecycle.record_submission_failure(error)
        })
    }

    /// BIR may have the return: keep the claim (so it is never sent again
    /// automatically) and record why, for the person to check.
    pub(crate) fn note_claimed_queueable_outcome_unknown<F: QueueableForm>(
        &self,
        tin: &str,
        taxable_year: u16,
        period: i64,
        claim_token: &str,
        error: String,
    ) -> Result<bool, DbError> {
        self.update_claimed_queueable::<F>(tin, taxable_year, period, claim_token, |lifecycle| {
            lifecycle.submission_error = Some(error)
        })
    }

    fn update_claimed_queueable<F: QueueableForm>(
        &self,
        tin: &str,
        taxable_year: u16,
        period: i64,
        claim_token: &str,
        change: impl FnOnce(&mut crate::forms::queueable::SubmissionLifecycle),
    ) -> Result<bool, DbError> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let Some((id, raw_json, db_status)) =
            select_row(&tx, F::FORM_CODE, tin, taxable_year, period)?
        else {
            return Ok(false);
        };
        let mut draft: F = serde_json::from_str(&raw_json)?;
        if db_status != "Queued"
            || !matches!(draft.lifecycle().status, FilingStatus::Queued)
            || draft.lifecycle().submission_claim_token.as_deref() != Some(claim_token)
        {
            return Ok(false);
        }
        change(draft.lifecycle_mut());
        let json = serde_json::to_string(&draft)?;
        let updated = tx.execute(
            "UPDATE form_drafts SET status = ?1, data_json = ?2, updated_at = datetime('now')
             WHERE id = ?3 AND status = 'Queued' AND data_json = ?4",
            params![
                filing_status_to_db(&draft.lifecycle().status),
                json,
                id,
                raw_json
            ],
        )?;
        if updated != 1 {
            return Ok(false);
        }
        tx.commit()?;
        let _ = self.request_post_commit_refresh();
        Ok(true)
    }

    /// Finish a network claim as Submitted. The claim must still belong to
    /// this worker and the reviewed fields must be unchanged.
    pub(crate) fn finish_claimed_queueable_submission<F: QueueableForm>(
        &self,
        draft: &F,
        claim_token: &str,
    ) -> Result<i64, DbError> {
        if !matches!(draft.lifecycle().status, FilingStatus::Sent) {
            return Err(DbError::Other(format!(
                "Only a Submitted {} snapshot can finish a network claim",
                F::FORM_CODE
            )));
        }
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let Some((id, raw_json, db_status)) = select_row(
            &tx,
            F::FORM_CODE,
            draft.tin(),
            draft.taxable_year(),
            draft.period_column(),
        )?
        else {
            return Err(DbError::Other(format!(
                "Claimed {} draft disappeared before the network attempt finished",
                F::FORM_CODE
            )));
        };
        let existing: F = serde_json::from_str(&raw_json)?;
        let lifecycle = existing.lifecycle();
        if db_status != "Queued"
            || !matches!(lifecycle.status, FilingStatus::Queued)
            || lifecycle.submission_claim_token.as_deref() != Some(claim_token)
            || lifecycle.submission_claimed_at.is_none()
        {
            return Err(DbError::Other(format!(
                "{} submission claim no longer belongs to this worker",
                F::FORM_CODE
            )));
        }
        if draft.lifecycle().queued_submission_fingerprint
            != lifecycle.queued_submission_fingerprint
            || draft.field_map() != existing.field_map()
        {
            return Err(DbError::Other(format!(
                "Claimed {} submission fields changed before completion",
                F::FORM_CODE
            )));
        }
        let expected_filename = existing.submission_filename();
        if draft.lifecycle().submission_filename.as_deref() != Some(expected_filename.as_str())
            || draft.lifecycle().submitted_at.is_none()
        {
            return Err(DbError::Other(format!(
                "Claimed {} submission completion did not keep the reviewed IAF filename and timestamp",
                F::FORM_CODE
            )));
        }
        let json = serde_json::to_string(draft)?;
        let updated = tx.execute(
            "UPDATE form_drafts SET status = 'Submitted', data_json = ?1, updated_at = datetime('now')
             WHERE id = ?2 AND status = 'Queued' AND data_json = ?3",
            params![json, id, raw_json],
        )?;
        if updated != 1 {
            return Err(DbError::Other(format!(
                "{} claim changed before submission completion was recorded",
                F::FORM_CODE
            )));
        }
        tx.commit()?;
        let _ = self.request_post_commit_refresh();
        Ok(id)
    }

    /// CAS-advance one exact Submitted snapshot to Confirmed.
    pub fn save_confirmed_queueable<F: QueueableForm>(&self, draft: &F) -> Result<i64, DbError> {
        let lifecycle = draft.lifecycle();
        if !matches!(lifecycle.status, FilingStatus::Confirmed)
            || lifecycle.confirmed_at.is_none()
            || !lifecycle.is_unclaimed()
        {
            return Err(DbError::Other(format!(
                "Only a completed {} confirmation may use the confirmation path",
                F::FORM_CODE
            )));
        }
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let Some((id, raw_json, db_status)) = select_row(
            &tx,
            F::FORM_CODE,
            draft.tin(),
            draft.taxable_year(),
            draft.period_column(),
        )?
        else {
            return Err(DbError::Other(format!(
                "Submitted {} draft disappeared before confirmation",
                F::FORM_CODE
            )));
        };
        let existing: F = serde_json::from_str(&raw_json)?;
        if db_status != "Submitted"
            || !matches!(existing.lifecycle().status, FilingStatus::Sent)
            || existing.field_map() != draft.field_map()
        {
            return Err(DbError::Other(format!(
                "Only the exact Submitted {} snapshot can be confirmed",
                F::FORM_CODE
            )));
        }
        let json = serde_json::to_string(draft)?;
        let updated = tx.execute(
            "UPDATE form_drafts SET status = 'Confirmed', data_json = ?1, updated_at = datetime('now')
             WHERE id = ?2 AND status = 'Submitted' AND data_json = ?3",
            params![json, id, raw_json],
        )?;
        if updated != 1 {
            return Err(DbError::Other(format!(
                "{} changed before its confirmation was recorded",
                F::FORM_CODE
            )));
        }
        tx.commit()?;
        let _ = self.request_post_commit_refresh();
        Ok(id)
    }

    /// Confirm a Submitted return from a matching BIR receipt.
    /// The Submitted draft whose uploaded filename is the receipt's (the
    /// `#email#` part and case are ignored; BIR echoes the base name).
    fn find_submitted_queueable_by_filename<F: QueueableForm>(
        &self,
        receipt_filename: &str,
    ) -> Result<Option<F>, DbError> {
        fn base(name: &str) -> String {
            let name = name.rsplit(['/', '\\']).next().unwrap_or(name);
            let name = name.split('#').next().unwrap_or(name);
            name.trim_end_matches(".xml").to_ascii_lowercase()
        }
        let wanted = base(receipt_filename);
        if wanted.is_empty() {
            return Ok(None);
        }
        let mut stmt = self.conn.prepare(
            "SELECT data_json FROM form_drafts WHERE form_code = ?1 AND status IN ('Submitted', 'Filed')",
        )?;
        let rows = stmt.query_map(params![F::FORM_CODE], |row| row.get::<_, String>(0))?;
        for raw in rows {
            let draft: F = serde_json::from_str(&raw?)?;
            if draft
                .lifecycle()
                .submission_filename
                .as_deref()
                .is_some_and(|name| base(name) == wanted)
            {
                return Ok(Some(draft));
            }
        }
        Ok(None)
    }

    pub fn confirm_queueable_from_receipt<F: QueueableForm>(
        &self,
        receipt: &SubmissionReceipt,
    ) -> Result<ReceiptConfirmationOutcome, DbError> {
        let by_period = match F::parse_period_code(&receipt.period) {
            Some((year, period)) => {
                let column = crate::forms::queueable::period_column(&period);
                self.get_queueable_draft::<F>(&receipt.tin, year, column)?
                    .filter(|draft| matches!(draft.lifecycle().status, FilingStatus::Sent))
            }
            None => None,
        };
        // Event-based forms (1706, 1800, …) key drafts by the dashboard's
        // open-ended slot while the official filename carries a date (and a
        // TCT number), so the period alone can't find them. Match the exact
        // filename the worker uploaded instead.
        let draft = match by_period {
            Some(draft) => Some(draft),
            None => self.find_submitted_queueable_by_filename::<F>(&receipt.filename)?,
        };
        let Some(mut draft) = draft else {
            return Ok(ReceiptConfirmationOutcome::Ignored);
        };
        if !matches!(draft.lifecycle().status, FilingStatus::Sent) {
            return Ok(ReceiptConfirmationOutcome::Ignored);
        }
        let receipt_id = receipt.id.ok_or_else(|| {
            DbError::Other(format!(
                "A {} receipt must be persisted before it can confirm a submission",
                F::FORM_CODE
            ))
        })?;
        let submitted_at = draft.lifecycle().submitted_at.clone().ok_or_else(|| {
            DbError::Other(format!(
                "Submitted {} draft has no submission timestamp",
                F::FORM_CODE
            ))
        })?;
        chrono::DateTime::parse_from_rfc3339(&submitted_at).map_err(|error| {
            DbError::Other(format!(
                "Submitted {} draft has an invalid submission timestamp: {error}",
                F::FORM_CODE
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
            .lifecycle()
            .queue_authorization
            .as_ref()
            .map(|auth| auth.authorized_at.clone());
        if !crate::filing_queue::receipt_belongs_to_this_generation(
            receipt_dt,
            authorized_at.as_deref(),
            &submitted_at,
        ) {
            return Ok(ReceiptConfirmationOutcome::Ignored);
        }
        draft.lifecycle_mut().transition_to_confirmed(
            date_str,
            Some(receipt_id),
            Some(receipt.filename.clone()),
        );
        self.save_confirmed_queueable(&draft)?;
        Ok(ReceiptConfirmationOutcome::Confirmed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forms::queueable::test_support::TestForm;

    #[test]
    fn a_legacy_row_with_a_null_period_column_is_adopted_not_duplicated() {
        let db = Database::open_in_memory_for_tests().unwrap();
        let mut form = TestForm::new(6, 10.0);
        let legacy = serde_json::to_string(&form).unwrap();
        db.conn
            .execute(
                "INSERT INTO form_drafts
                    (tin, form_code, taxable_year, quarter, period_key, status, data_json)
                 VALUES (?1, ?2, 2025, NULL, 'M06', 'Draft', ?3)",
                params![form.tin, TestForm::CODE, legacy],
            )
            .unwrap();
        form.amount = 11.0;
        db.save_queueable_draft(&form).unwrap();
        let rows: Vec<(Option<i64>, String)> = db
            .conn
            .prepare("SELECT quarter, data_json FROM form_drafts WHERE form_code = ?1")
            .unwrap()
            .query_map(params![TestForm::CODE], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, Some(6));
        let stored: TestForm = serde_json::from_str(&rows[0].1).unwrap();
        assert_eq!(stored.amount, 11.0);
        assert_eq!(
            db.get_queueable_draft::<TestForm>(&form.tin, 2025, 6)
                .unwrap()
                .map(|f| f.amount),
            Some(11.0)
        );
    }

    fn rows(db: &Database) -> Vec<(i64, i64, String)> {
        db.conn
            .prepare("SELECT id, quarter, status FROM form_drafts WHERE form_code = ?1 ORDER BY id")
            .unwrap()
            .query_map(params![TestForm::CODE], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    #[test]
    fn a_rekeyed_draft_moves_its_row() {
        let db = Database::open_in_memory_for_tests().unwrap();
        let id = db.save_queueable_draft(&TestForm::new(6, 10.0)).unwrap();
        let mut loaded = db
            .get_queueable_draft::<TestForm>("123-456-788-00000", 2025, 6)
            .unwrap()
            .unwrap();
        assert_eq!(loaded.id, Some(id));
        loaded.month = 7;
        loaded.amount = 12.0;
        assert_eq!(db.save_queueable_draft(&loaded).unwrap(), id);
        assert_eq!(rows(&db), vec![(id, 7, "Draft".to_string())]);
        let moved = db
            .get_queueable_draft::<TestForm>("123-456-788-00000", 2025, 7)
            .unwrap()
            .unwrap();
        assert_eq!((moved.id, moved.amount), (Some(id), 12.0));
    }

    #[test]
    fn a_rekey_never_touches_a_non_draft_old_row() {
        let db = Database::open_in_memory_for_tests().unwrap();
        let id = db.save_queueable_draft(&TestForm::new(6, 10.0)).unwrap();
        db.conn
            .execute(
                "UPDATE form_drafts SET status = 'Filed' WHERE id = ?1",
                params![id],
            )
            .unwrap();
        let mut copy = TestForm::new(7, 12.0);
        copy.id = Some(id);
        let new_id = db.save_queueable_draft(&copy).unwrap();
        assert_ne!(new_id, id);
        assert_eq!(
            rows(&db),
            vec![
                (id, 6, "Filed".to_string()),
                (new_id, 7, "Draft".to_string())
            ]
        );
    }

    #[test]
    fn a_rekey_onto_another_draft_is_refused() {
        let db = Database::open_in_memory_for_tests().unwrap();
        let first = db.save_queueable_draft(&TestForm::new(6, 10.0)).unwrap();
        let second = db.save_queueable_draft(&TestForm::new(7, 20.0)).unwrap();
        let mut loaded = db
            .get_queueable_draft::<TestForm>("123-456-788-00000", 2025, 6)
            .unwrap()
            .unwrap();
        loaded.month = 7;
        let error = db.save_queueable_draft(&loaded).unwrap_err();
        assert!(error.to_string().contains(REKEY_COLLISION_MESSAGE));
        assert_eq!(
            rows(&db),
            vec![
                (first, 6, "Draft".to_string()),
                (second, 7, "Draft".to_string())
            ]
        );
        let kept = db
            .get_queueable_draft::<TestForm>("123-456-788-00000", 2025, 7)
            .unwrap()
            .unwrap();
        assert_eq!(kept.amount, 20.0);
    }

    const TIN: &str = "123-456-788-00000";

    /// Store `form` directly with its own status, as the worker would.
    fn store(db: &Database, form: &TestForm) -> i64 {
        let id = db
            .save_queueable_draft(&TestForm::new(form.month, 0.0))
            .unwrap();
        let mut stamped = form.clone();
        stamped.id = Some(id);
        db.conn
            .execute(
                "UPDATE form_drafts SET status = ?1, data_json = ?2 WHERE id = ?3",
                params![
                    filing_status_to_db(&form.lifecycle.status),
                    serde_json::to_string(&stamped).unwrap(),
                    id
                ],
            )
            .unwrap();
        id
    }

    fn queued() -> TestForm {
        let mut form = TestForm::new(6, 10.0);
        form.queue(crate::filing_queue::QueueAuthSource::Gui)
            .unwrap();
        form
    }

    fn submitted() -> TestForm {
        let mut form = queued();
        let filename = form.submission_filename();
        form.lifecycle.transition_to_submitted(filename);
        form
    }

    fn confirmed() -> TestForm {
        let mut form = submitted();
        form.lifecycle
            .transition_to_confirmed("2025-07-01T08:00:00".to_string(), Some(42), None);
        form
    }

    fn snapshots(db: &Database) -> Vec<crate::db::AmendmentSnapshot> {
        db.list_amendment_snapshots(TestForm::CODE, TIN, 2025, "M06")
            .unwrap()
    }

    #[test]
    fn reverting_a_queued_return_is_a_plain_revert() {
        let db = Database::open_in_memory_for_tests().unwrap();
        let id = store(&db, &queued());
        let result = db
            .revert_queueable_for_amendment::<TestForm>(TIN, 2025, 6)
            .unwrap();
        assert_eq!(result.previous_status, FilingStatus::Queued);
        assert_eq!(result.snapshot_id, None);
        assert!(!result.marked_amended);
        assert!(!result.draft.is_amended);
        assert_eq!(rows(&db), vec![(id, 6, "Draft".to_string())]);
        let stored = db
            .get_queueable_draft::<TestForm>(TIN, 2025, 6)
            .unwrap()
            .unwrap();
        assert!(stored.lifecycle.is_editable());
        assert!(stored.lifecycle.submitted_payload.is_none());
        assert!(snapshots(&db).is_empty());
    }

    #[test]
    fn reverting_a_submitted_return_snapshots_it_and_marks_an_amendment() {
        let db = Database::open_in_memory_for_tests().unwrap();
        let form = submitted();
        let id = store(&db, &form);
        let result = db
            .revert_queueable_for_amendment::<TestForm>(TIN, 2025, 6)
            .unwrap();
        assert_eq!(result.previous_status, FilingStatus::Sent);
        assert!(result.marked_amended);
        assert_eq!(rows(&db), vec![(id, 6, "Draft".to_string())]);
        let stored = db
            .get_queueable_draft::<TestForm>(TIN, 2025, 6)
            .unwrap()
            .unwrap();
        assert!(stored.is_amended);
        assert!(stored.lifecycle.is_editable());
        assert_eq!(stored.id, Some(id));

        let kept = snapshots(&db);
        assert_eq!(kept.len(), 1);
        assert_eq!(Some(kept[0].id), result.snapshot_id);
        assert_eq!(kept[0].status_at_revert, "Submitted");
        assert_eq!(kept[0].submitted_at, form.lifecycle.submitted_at);
        assert_eq!(kept[0].confirmed_at, None);
        assert_eq!(
            kept[0].submission_filename,
            form.lifecycle.submission_filename
        );
        assert_eq!(kept[0].payload_source, SnapshotPayloadSource::Stored);
        assert_eq!(kept[0].submitted_xml, form.lifecycle.submitted_payload);
        assert_eq!(kept[0].period_key, "M06");
    }

    #[test]
    fn reverting_a_confirmed_return_keeps_its_receipt_and_every_earlier_snapshot() {
        let db = Database::open_in_memory_for_tests().unwrap();
        store(&db, &confirmed());
        db.revert_queueable_for_amendment::<TestForm>(TIN, 2025, 6)
            .unwrap();

        // File the amendment, then revert it again: both snapshots remain.
        let mut amended = db
            .get_queueable_draft::<TestForm>(TIN, 2025, 6)
            .unwrap()
            .unwrap();
        amended.amount = 12.0;
        amended
            .queue(crate::filing_queue::QueueAuthSource::Gui)
            .unwrap();
        let filename = amended.submission_filename();
        amended.lifecycle.transition_to_submitted(filename);
        let json = serde_json::to_string(&amended).unwrap();
        db.conn
            .execute(
                "UPDATE form_drafts SET status = 'Submitted', data_json = ?1",
                params![json],
            )
            .unwrap();
        db.revert_queueable_for_amendment::<TestForm>(TIN, 2025, 6)
            .unwrap();

        let kept = snapshots(&db);
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0].status_at_revert, "Confirmed");
        assert_eq!(kept[0].receipt_id, Some(42));
        assert_eq!(kept[0].confirmed_at.as_deref(), Some("2025-07-01T08:00:00"));
        assert!(
            kept[0]
                .submitted_xml
                .as_deref()
                .unwrap()
                .contains("txtTax14=10.00")
        );
        assert!(
            kept[1]
                .submitted_xml
                .as_deref()
                .unwrap()
                .contains("txtTax14=12.00")
        );
        assert_eq!(rows(&db).len(), 1);
    }

    #[test]
    fn an_older_return_without_stored_plaintext_gets_a_regenerated_copy() {
        let db = Database::open_in_memory_for_tests().unwrap();
        let mut form = confirmed();
        form.lifecycle.submitted_payload = None;
        store(&db, &form);
        db.revert_queueable_for_amendment::<TestForm>(TIN, 2025, 6)
            .unwrap();
        let kept = snapshots(&db);
        assert_eq!(kept[0].payload_source, SnapshotPayloadSource::Regenerated);
        assert_eq!(
            kept[0].submitted_xml.as_deref(),
            Some(form.official_payload().unwrap().as_str())
        );
    }

    #[test]
    fn a_paid_or_draft_return_is_refused_and_nothing_changes() {
        let db = Database::open_in_memory_for_tests().unwrap();
        let mut paid = confirmed();
        paid.lifecycle.status = FilingStatus::Paid;
        let id = store(&db, &paid);
        let error = db
            .revert_queueable_for_amendment::<TestForm>(TIN, 2025, 6)
            .unwrap_err();
        assert!(error.to_string().contains("Paid"), "{error}");
        assert_eq!(rows(&db), vec![(id, 6, "Paid".to_string())]);
        assert!(snapshots(&db).is_empty());

        db.save_queueable_draft(&TestForm::new(7, 1.0)).unwrap();
        assert!(
            db.revert_queueable_for_amendment::<TestForm>(TIN, 2025, 7)
                .is_err()
        );
    }

    #[test]
    fn a_failed_save_rolls_back_the_snapshot() {
        let db = Database::open_in_memory_for_tests().unwrap();
        let id = store(&db, &submitted());
        db.conn
            .execute_batch(
                "CREATE TRIGGER block_revert BEFORE UPDATE ON form_drafts
                 BEGIN SELECT RAISE(ABORT, 'blocked'); END;",
            )
            .unwrap();
        assert!(
            db.revert_queueable_for_amendment::<TestForm>(TIN, 2025, 6)
                .is_err()
        );
        assert!(snapshots(&db).is_empty());
        assert_eq!(rows(&db), vec![(id, 6, "Submitted".to_string())]);
    }
}
