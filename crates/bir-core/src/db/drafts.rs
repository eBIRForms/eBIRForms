//! Form drafts repository — save, load, and list tax form drafts.

use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};

use super::{Database, DbError};
use crate::forms::form_1601c::Form1601CDraft;
use crate::forms::form_2551q::{
    AnnualIncomeTaxElection, Form2551QDraft, Item13Election, annual_income_tax_election,
};
use crate::forms::{
    FilingFrequency, FilingPeriod, FilingStatus, FormDraftSummary, FormFilingProgress,
    QuarterState, find_form,
};
use crate::profile::TaxpayerProfile;

pub enum Claim2551QSubmissionResult {
    Claimed {
        draft: Form2551QDraft,
        token: String,
    },
    Rejected {
        draft: Form2551QDraft,
        errors: Vec<(String, String)>,
    },
    Superseded,
}

pub enum Claim1601CSubmissionResult {
    Claimed {
        draft: Form1601CDraft,
        token: String,
    },
    Rejected {
        draft: Form1601CDraft,
        errors: Vec<(String, String)>,
    },
    Superseded,
}

/// Only allow-listed reason for a human-confirmed abandoned-claim release.
/// The outer agent must pass this after a person confirms nothing reached BIR.
pub const ABANDONED_CLAIM_RELEASE_REASON: &str = "abandoned_no_bir_filing";

/// Result of a fail-closed abandoned-claim release. Does not queue or file.
pub enum AbandonedClaimRelease<T> {
    Released {
        previous_status: FilingStatus,
        previous_claim_present: bool,
        previous_claimed_at: Option<String>,
        draft: T,
        reason: String,
    },
    AlreadyClear {
        previous_status: Option<FilingStatus>,
        draft: Option<T>,
        reason: String,
    },
}

pub(super) fn abandoned_claim_audit(reason: &str) -> String {
    format!("Abandoned claim released ({reason}); human confirmed nothing reached BIR")
}

pub(super) fn require_abandoned_release_reason(reason: &str) -> Result<(), DbError> {
    if reason != ABANDONED_CLAIM_RELEASE_REASON {
        return Err(DbError::Other(format!(
            "Abandoned claim release requires reason `{ABANDONED_CLAIM_RELEASE_REASON}`"
        )));
    }
    Ok(())
}

pub(super) fn claim_fields_present(token: &Option<String>, claimed_at: &Option<String>) -> bool {
    token.is_some() && claimed_at.is_some()
}

pub(super) fn claim_fields_partial(token: &Option<String>, claimed_at: &Option<String>) -> bool {
    token.is_some() != claimed_at.is_some()
}

pub(super) fn filing_status_to_db(status: &FilingStatus) -> &'static str {
    match status {
        FilingStatus::Draft => "Draft",
        FilingStatus::Queued => "Queued",
        FilingStatus::Sent => "Submitted",
        FilingStatus::Confirmed => "Confirmed",
        FilingStatus::Paid => "Paid",
    }
}

pub(super) fn filing_status_from_db(status: &str) -> FilingStatus {
    match status {
        "Confirmed" => FilingStatus::Confirmed,
        "Submitted" | "Filed" => FilingStatus::Sent,
        "Paid" => FilingStatus::Paid,
        "Queued" => FilingStatus::Queued,
        _ => FilingStatus::Draft,
    }
}

fn quarter_state_from_db(status: &str) -> QuarterState {
    match status {
        "Confirmed" => QuarterState::Confirmed,
        "Submitted" | "Filed" => QuarterState::Sent,
        "Paid" => QuarterState::Paid,
        "Queued" => QuarterState::Queued,
        "Draft" => QuarterState::Draft,
        _ => QuarterState::Draft,
    }
}

/// How far a return has gone, for picking one state per month tile.
fn state_rank(state: &QuarterState) -> u8 {
    match state {
        QuarterState::NotStarted => 0,
        QuarterState::Draft => 1,
        QuarterState::Queued => 2,
        QuarterState::Sent => 3,
        QuarterState::Confirmed => 4,
        QuarterState::Paid => 5,
    }
}

fn counts_as_started_or_filed(state: &QuarterState) -> bool {
    matches!(
        state,
        QuarterState::Queued | QuarterState::Sent | QuarterState::Confirmed | QuarterState::Paid
    )
}

/// Record one return's state on the tile its period owns; several rows can
/// name one period (a NULL-quarter row next to one with a legacy quarter), so
/// the most advanced return wins rather than whichever row came last.
fn apply_period_state(
    progress: &mut FormFilingProgress,
    monthly: bool,
    period: FilingPeriod,
    state: QuarterState,
) {
    let keep_most_advanced = |slot: &mut QuarterState, state: QuarterState| {
        if state_rank(&state) > state_rank(slot) {
            *slot = state;
        }
    };
    match period {
        FilingPeriod::Monthly(month) => {
            let idx = usize::from(month.clamp(1, 12) - 1);
            keep_most_advanced(&mut progress.months[idx], state);
        }
        FilingPeriod::Quarterly(quarter) => {
            let idx = usize::from(quarter.clamp(1, 4) - 1);
            keep_most_advanced(&mut progress.quarters[idx], state);
        }
        FilingPeriod::Annual => {
            keep_most_advanced(&mut progress.annual_status, state);
        }
        // A monthly form keyed by its Item 1 date (`MMDD`, e.g. 2200-AN) shows
        // on its month's tile; the most advanced return of that month wins.
        FilingPeriod::OpenEnded(mmdd) if monthly && (1..=12).contains(&(mmdd / 100)) => {
            let idx = (mmdd / 100 - 1) as usize;
            if state_rank(&state) > state_rank(&progress.months[idx]) {
                progress.months[idx] = state;
            }
        }
        FilingPeriod::OpenEnded(_) => {
            if counts_as_started_or_filed(&state) {
                progress.open_ended_count = progress.open_ended_count.saturating_add(1);
            }
        }
    }
}

/// Map a legacy filename period (`062025`, `2026Q1`, `122026Q1`, `2025`,
/// `03152026`) onto the progress slot this form uses. `None` when it cannot
/// name a period of this form, so an unmatched record never shows as filed.
fn legacy_return_period(form_code: &str, period: &str, year: u16) -> Option<FilingPeriod> {
    let (digits, quarter) = match period.find(['Q', 'q']) {
        Some(index) => (
            period.get(..index)?,
            Some(
                period[index + 1..]
                    .parse::<u8>()
                    .ok()
                    .filter(|quarter| (1..=4).contains(quarter))?,
            ),
        ),
        None => (period, None),
    };
    if !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    if digits.len() == 8 {
        if quarter.is_some() {
            return None;
        }
        let month = digits[..2]
            .parse::<u8>()
            .ok()
            .filter(|month| (1..=12).contains(month))?;
        let day = digits[2..4]
            .parse::<u8>()
            .ok()
            .filter(|day| (1..=31).contains(day))?;
        if digits[4..].parse::<u16>().ok()? != year {
            return None;
        }
        chrono::NaiveDate::from_ymd_opt(i32::from(year), u32::from(month), u32::from(day))?;
        return match frequency_for_form(form_code) {
            FilingFrequency::Monthly | FilingFrequency::OpenEnded => Some(FilingPeriod::OpenEnded(
                u32::from(month) * 100 + u32::from(day),
            )),
            _ => None,
        };
    }
    let (period_year, month) = match digits.len() {
        4 => (digits.parse::<u16>().ok()?, None),
        6 => (
            digits[2..].parse::<u16>().ok()?,
            Some(
                digits[..2]
                    .parse::<u8>()
                    .ok()
                    .filter(|month| (1..=12).contains(month))?,
            ),
        ),
        _ => return None,
    };
    if period_year != year {
        return None;
    }
    match (frequency_for_form(form_code), quarter, month) {
        (FilingFrequency::Monthly, _, Some(month)) => Some(FilingPeriod::Monthly(month)),
        (FilingFrequency::Quarterly, Some(quarter), _) => Some(FilingPeriod::Quarterly(quarter)),
        (FilingFrequency::Quarterly, None, Some(month)) => {
            Some(FilingPeriod::Quarterly(month.div_ceil(3)))
        }
        (FilingFrequency::Annual, _, _) => Some(FilingPeriod::Annual),
        (FilingFrequency::OpenEnded, quarter, month) => Some(FilingPeriod::OpenEnded(
            u32::from(quarter.map(|quarter| quarter * 3).or(month).unwrap_or(0)) * 100,
        )),
        _ => None,
    }
}

const AUDITED_2551Q_RECEIPT_FORM: &str = "2551Qv2018";

fn matches_2551q_receipt_filename(submitted_filename: &str, receipt_filename: &str) -> bool {
    let Some(receipt_stem_with_identity) = receipt_filename.strip_suffix(".xml") else {
        return false;
    };
    if receipt_stem_with_identity.is_empty() {
        return false;
    }
    let Some(submitted_stem) = submitted_filename.strip_suffix(".xml") else {
        return false;
    };
    let Some((receipt_stem, email_suffix)) = submitted_stem.split_once('#') else {
        return false;
    };
    let Some(email) = email_suffix.strip_suffix('#') else {
        return false;
    };
    if receipt_stem.is_empty() || email.is_empty() || email.contains('#') {
        return false;
    }

    let submitted_identity = crate::receipt::split_bir_filename(&format!("{receipt_stem}.xml"));
    let receipt_identity = crate::receipt::split_bir_filename(receipt_filename);
    matches!(
        (submitted_identity, receipt_identity),
        (
            Some((submitted_tin, submitted_form, submitted_period)),
            Some((receipt_tin, receipt_form, receipt_period))
        ) if submitted_tin == receipt_tin
            && submitted_form == AUDITED_2551Q_RECEIPT_FORM
            && super::receipts::is_audited_2551q_receipt_form_type(&receipt_form)
            && submitted_period == receipt_period
    )
}

fn parse_2551q_receipt_timestamp(
    received_date: &str,
    received_time: &str,
) -> Option<chrono::DateTime<chrono::FixedOffset>> {
    let received = chrono::NaiveDateTime::parse_from_str(
        &format!("{received_date}T{received_time}"),
        "%Y-%m-%dT%H:%M:%S",
    )
    .ok()?;
    let philippine_time = chrono::FixedOffset::east_opt(8 * 60 * 60)?;
    received.and_local_timezone(philippine_time).single()
}

fn frequency_for_form(form_code: &str) -> FilingFrequency {
    find_form(form_code)
        .map(|form| form.frequency.clone())
        .unwrap_or_else(|| match form_code {
            "0619E" | "0619F" | "1601C" => FilingFrequency::Monthly,
            "2551Q" | "2550Q" | "1701Q" => FilingFrequency::Quarterly,
            "1701" | "1702RT" | "1702MX" => FilingFrequency::Annual,
            "0605" => FilingFrequency::OpenEnded,
            _ => FilingFrequency::Quarterly,
        })
}

fn normalize_month(slot: Option<u8>) -> u8 {
    slot.unwrap_or(1).clamp(1, 12)
}

fn normalize_quarter(slot: Option<u8>) -> u8 {
    slot.unwrap_or(1).clamp(1, 4)
}

fn slot_from_i64(slot: Option<i64>) -> Option<u8> {
    slot.and_then(|value| u8::try_from(value).ok())
}

pub(super) fn period_from_legacy_slot(
    form_code: &str,
    slot: Option<u8>,
    default_open_ended_key: u32,
) -> FilingPeriod {
    match frequency_for_form(form_code) {
        FilingFrequency::Monthly => FilingPeriod::Monthly(normalize_month(slot)),
        FilingFrequency::Quarterly => FilingPeriod::Quarterly(normalize_quarter(slot)),
        FilingFrequency::Annual => FilingPeriod::Annual,
        FilingFrequency::OpenEnded => {
            let key = slot
                .map(u32::from)
                .filter(|value| *value > 0)
                .unwrap_or(default_open_ended_key);
            FilingPeriod::OpenEnded(key)
        }
    }
}

fn period_from_row(
    form_code: &str,
    legacy_slot: Option<i64>,
    period_key: Option<&str>,
) -> FilingPeriod {
    if let Some(period) = period_key.and_then(FilingPeriod::from_period_key) {
        return period;
    }
    period_from_legacy_slot(form_code, slot_from_i64(legacy_slot), 1)
}

fn legacy_slot_for_period(period: &FilingPeriod) -> Option<i64> {
    match period {
        FilingPeriod::Monthly(month) => Some(i64::from(*month)),
        FilingPeriod::Quarterly(quarter) => Some(i64::from(*quarter)),
        FilingPeriod::Annual | FilingPeriod::OpenEnded(_) => None,
    }
}

fn summary_period_fields(period: &FilingPeriod) -> (Option<u8>, Option<u8>) {
    match period {
        FilingPeriod::Monthly(month) => (None, Some(*month)),
        FilingPeriod::Quarterly(quarter) => (Some(*quarter), None),
        FilingPeriod::Annual | FilingPeriod::OpenEnded(_) => (None, None),
    }
}

/// One `form_drafts` row (`id, tin, form_code, taxable_year, quarter,
/// period_key, status, updated_at`) as a summary.
fn draft_summary_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<FormDraftSummary> {
    let form_code: String = row.get(2)?;
    let period_val = row.get::<_, Option<i64>>(4)?.map(|q| q as u8);
    let legacy_slot = row.get::<_, Option<i64>>(4)?;
    let period_key = row.get::<_, Option<String>>(5)?;
    let period = period_from_row(&form_code, legacy_slot, period_key.as_deref());
    let (quarter, month) = summary_period_fields(&period);
    let frequency = frequency_for_form(&form_code);
    let quarter = quarter.or({
        if matches!(&frequency, FilingFrequency::Quarterly) {
            period_val
        } else {
            None
        }
    });
    let month = month.or({
        if matches!(&frequency, FilingFrequency::Monthly) {
            period_val
        } else {
            None
        }
    });

    Ok(FormDraftSummary {
        id: row.get(0)?,
        tin: row.get(1)?,
        form_code,
        taxable_year: row.get::<_, i64>(3)? as u16,
        quarter,
        month,
        status: filing_status_from_db(row.get::<_, String>(6)?.as_str()),
        updated_at: row.get(7)?,
    })
}

impl Database {
    fn next_open_ended_period_number(
        &self,
        tin: &str,
        form_code: &str,
        year: u16,
    ) -> Result<u32, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT period_key FROM form_drafts
             WHERE tin = ?1 AND form_code = ?2 AND taxable_year = ?3
               AND period_key LIKE 'O%'",
        )?;
        let rows = stmt.query_map(params![tin, form_code, year as i64], |row| {
            row.get::<_, String>(0)
        })?;

        let mut max_key = 0;
        for row in rows {
            if let Some(FilingPeriod::OpenEnded(key)) = FilingPeriod::from_period_key(&row?) {
                max_key = max_key.max(key);
            }
        }

        Ok(max_key.saturating_add(1).max(1))
    }

    /// Save or update a generic form draft.
    /// Uses `period_key` as the real period identity and keeps the legacy
    /// `quarter` column populated only for monthly/quarterly compatibility.
    pub fn save_form_draft<T: serde::Serialize>(
        &self,
        tin: &str,
        form_code: &str,
        year: u16,
        quarter: Option<u8>,
        status: &FilingStatus,
        draft: &T,
    ) -> Result<i64, DbError> {
        let default_open_ended_key =
            if matches!(frequency_for_form(form_code), FilingFrequency::OpenEnded)
                && quarter.is_none_or(|value| value == 0)
            {
                self.next_open_ended_period_number(tin, form_code, year)?
            } else {
                1
            };
        let period = period_from_legacy_slot(form_code, quarter, default_open_ended_key);
        self.save_form_draft_v2(tin, form_code, year, &period, status, draft)
    }

    /// The stored row [`Self::save_form_draft`] would replace for these
    /// arguments: `None` when it would add a new open-ended entry (an
    /// open-ended form saved with no entry, or entry 0).
    pub fn form_draft_for_save<T: serde::de::DeserializeOwned>(
        &self,
        tin: &str,
        form_code: &str,
        year: u16,
        quarter: Option<u8>,
    ) -> Result<Option<T>, DbError> {
        if matches!(frequency_for_form(form_code), FilingFrequency::OpenEnded)
            && quarter.is_none_or(|value| value == 0)
        {
            return Ok(None);
        }
        let period = period_from_legacy_slot(form_code, quarter, 1);
        self.get_form_draft_v2(tin, form_code, year, &period)
    }

    /// Load a generic form draft for a specific (tin, form_code, year, quarter).
    pub fn get_form_draft<T: serde::de::DeserializeOwned>(
        &self,
        tin: &str,
        form_code: &str,
        year: u16,
        quarter: Option<u8>,
    ) -> Result<Option<T>, DbError> {
        let period = period_from_legacy_slot(form_code, quarter, 1);
        if let Some(draft) = self.get_form_draft_v2(tin, form_code, year, &period)? {
            return Ok(Some(draft));
        }

        let mut stmt;
        let mut rows = if let Some(q) = quarter {
            stmt = self.conn.prepare(
                "SELECT data_json FROM form_drafts
                 WHERE tin = ?1 AND form_code = ?2
                   AND taxable_year = ?3 AND quarter = ?4",
            )?;
            stmt.query(params![tin, form_code, year as i64, q as i64])?
        } else {
            stmt = self.conn.prepare(
                "SELECT data_json FROM form_drafts
                 WHERE tin = ?1 AND form_code = ?2
                   AND taxable_year = ?3 AND quarter IS NULL",
            )?;
            stmt.query(params![tin, form_code, year as i64])?
        };

        if let Some(row) = rows.next()? {
            let json: String = row.get(0)?;
            let draft: T = serde_json::from_str(&json)?;
            Ok(Some(draft))
        } else {
            Ok(None)
        }
    }

    /// Save or update an editable Form 2551Q draft.
    ///
    /// Generic saves may never create or replace Queued and later snapshots.
    /// Queue, cancellation, claim, and completion each use a dedicated CAS path.
    pub fn save_2551q_draft(&self, draft: &Form2551QDraft) -> Result<i64, DbError> {
        if !matches!(draft.status, FilingStatus::Draft)
            || draft.submission_claim_token.is_some()
            || draft.submission_claimed_at.is_some()
        {
            return Err(DbError::Other(
                "Only an editable Draft 2551Q return may use the draft save path".to_string(),
            ));
        }

        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let json = serde_json::to_string(draft)?;
        let quarter = i64::from(draft.quarter);
        let period_key = FilingPeriod::Quarterly(draft.quarter).to_period_key();
        let existing = tx
            .query_row(
                "SELECT id, status, data_json FROM form_drafts
                 WHERE tin = ?1 AND form_code = '2551Q'
                   AND taxable_year = ?2 AND quarter = ?3",
                params![&draft.tin, i64::from(draft.taxable_year), quarter],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?;

        let id = if let Some((id, db_status, raw_json)) = existing {
            if db_status != "Draft" {
                return Err(DbError::Other(
                    "An immutable queued or filed 2551Q snapshot cannot be replaced by an editable draft save"
                        .to_string(),
                ));
            }
            let stored: Form2551QDraft = serde_json::from_str(&raw_json)?;
            if !matches!(stored.status, FilingStatus::Draft)
                || stored.submission_claim_token.is_some()
                || stored.submission_claimed_at.is_some()
            {
                return Err(DbError::Other(
                    "An immutable queued or filed 2551Q snapshot cannot be replaced by an editable draft save"
                        .to_string(),
                ));
            }
            let updated = tx.execute(
                "UPDATE form_drafts
                 SET data_json = ?1, period_key = ?2, updated_at = datetime('now')
                 WHERE id = ?3 AND status = 'Draft' AND data_json = ?4",
                params![json, period_key, id, raw_json],
            )?;
            if updated != 1 {
                return Err(DbError::Other(
                    "2551Q draft changed before the editable save completed".to_string(),
                ));
            }
            id
        } else {
            tx.execute(
                "INSERT INTO form_drafts
                    (tin, form_code, taxable_year, quarter, period_key, status, data_json)
                 VALUES (?1, '2551Q', ?2, ?3, ?4, 'Draft', ?5)",
                params![
                    &draft.tin,
                    i64::from(draft.taxable_year),
                    quarter,
                    period_key,
                    json
                ],
            )?;
            tx.last_insert_rowid()
        };

        tx.commit()?;
        let _ = self.request_post_commit_refresh();
        Ok(id)
    }

    /// Reconcile only the profile-staleness audit fields on an immutable 2551Q
    /// snapshot.
    ///
    /// The caller may hold an older Queued/Submitted view while a background
    /// worker or receipt flow has already advanced the stored return. Reloading
    /// the row inside this transaction and CAS-updating its exact JSON prevents
    /// that older view from replacing newer submission, confirmation, payment,
    /// receipt, or claim data through the generic whole-row draft UPSERT.
    pub fn reconcile_immutable_2551q_profile_snapshot(
        &self,
        tin: &str,
        taxable_year: u16,
        quarter: u8,
        profile: &TaxpayerProfile,
    ) -> Result<Form2551QDraft, DbError> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let Some((raw_json, db_status)) = tx
            .query_row(
                "SELECT data_json, status FROM form_drafts
                 WHERE tin = ?1 AND form_code = '2551Q'
                   AND taxable_year = ?2 AND quarter = ?3",
                params![tin, i64::from(taxable_year), i64::from(quarter)],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?
        else {
            return Err(DbError::Other(format!(
                "Cannot reconcile 2551Q profile snapshot because {tin}/{taxable_year}/Q{quarter} no longer exists"
            )));
        };

        let mut current: Form2551QDraft = serde_json::from_str(&raw_json)?;
        if matches!(current.status, FilingStatus::Draft) {
            return Err(DbError::Other(
                "Editable 2551Q drafts must use the draft save path for profile reconciliation"
                    .to_string(),
            ));
        }
        if current.submission_claim_token.is_some() || current.submission_claimed_at.is_some() {
            return Err(DbError::Other(
                "A claimed 2551Q submission snapshot is frozen until the network attempt completes"
                    .to_string(),
            ));
        }
        if filing_status_from_db(&db_status) != current.status {
            return Err(DbError::Other(format!(
                "Stored 2551Q status column '{db_status}' does not match its immutable snapshot"
            )));
        }

        // Immutable reconciliation changes only audit/staleness fields in the
        // freshly loaded row. A resolution error is persisted in those fields
        // and returned through `profile_resolution_error` for the UI to show.
        let _ = current.reconcile_with_effective_profile(profile);
        let reconciled_json = serde_json::to_string(&current)?;
        let updated = tx.execute(
            "UPDATE form_drafts
             SET data_json = ?1, updated_at = datetime('now')
             WHERE tin = ?2 AND form_code = '2551Q'
               AND taxable_year = ?3 AND quarter = ?4
               AND status = ?5 AND data_json = ?6",
            params![
                reconciled_json,
                tin,
                i64::from(taxable_year),
                i64::from(quarter),
                db_status,
                raw_json
            ],
        )?;
        if updated != 1 {
            return Err(DbError::Other(
                "2551Q changed while its immutable profile marker was being reconciled".to_string(),
            ));
        }
        tx.commit()?;
        Ok(current)
    }

    /// Atomically persist a queued 2551Q draft and the initial-quarter Item 13
    /// election it makes.
    ///
    /// An 8% election on either an existing taxpayer's Q1 return or a new
    /// registrant's first return is part of the taxpayer's annual income-tax
    /// regime. A queued return must not become visible without the corresponding
    /// profile ledger entry. Conflicting same-year profile elections fail the
    /// entire transaction and leave both the profile and draft unchanged.
    pub fn save_queued_2551q_draft_and_election(
        &self,
        draft: &Form2551QDraft,
    ) -> Result<i64, DbError> {
        self.save_queued_2551q_draft_and_election_with_post_commit_status(draft)
            .map(super::PostCommitWrite::into_committed)
    }

    /// Atomically persist the queued draft and annual election while exposing
    /// the independent status of the post-commit calendar/deadline refresh.
    pub fn save_queued_2551q_draft_and_election_with_post_commit_status(
        &self,
        draft: &Form2551QDraft,
    ) -> Result<super::PostCommitWrite<i64>, DbError> {
        if !matches!(draft.status, FilingStatus::Queued) {
            return Err(DbError::Other(
                "Only a queued 2551Q draft can be saved through the submission path".to_string(),
            ));
        }
        if !draft.is_amended
            && self.has_filed_original(
                &draft.tin,
                "2551Q",
                draft.taxable_year,
                &FilingPeriod::Quarterly(draft.quarter).to_period_key(),
                draft.id,
            )?
        {
            return Err(DbError::AmendedReturnRequired);
        }
        if draft.submission_claim_token.is_some() || draft.submission_claimed_at.is_some() {
            return Err(DbError::Other(
                "A reviewed 2551Q queue snapshot cannot carry a network claim".to_string(),
            ));
        }
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        use crate::profile::{IncomeTaxElection, TaxElectionHistory, TaxpayerProfile};

        let existing = tx
            .query_row(
                "SELECT id, status, data_json FROM form_drafts
                 WHERE tin = ?1 AND form_code = '2551Q'
                   AND taxable_year = ?2 AND quarter = ?3",
                params![
                    &draft.tin,
                    i64::from(draft.taxable_year),
                    i64::from(draft.quarter)
                ],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?;
        if let Some((_, db_status, raw_json)) = &existing {
            if db_status != "Draft" {
                return Err(DbError::Other(
                    "2551Q must return to Draft through its dedicated CAS path before a new queue snapshot can replace it"
                        .to_string(),
                ));
            }
            let stored: Form2551QDraft = serde_json::from_str(raw_json)?;
            if !matches!(stored.status, FilingStatus::Draft)
                || stored.submission_claim_token.is_some()
                || stored.submission_claimed_at.is_some()
            {
                return Err(DbError::Other(
                    "2551Q must return to Draft through its dedicated CAS path before a new queue snapshot can replace it"
                        .to_string(),
                ));
            }
        }

        // Every queued return is bound to the authoritative profile, even if
        // Item 13 does not create an 8% ledger entry. Read it inside this
        // transaction so profile synchronization, election persistence, and
        // the draft UPSERT either all commit or all roll back.
        let profile_json = tx
            .query_row(
                "SELECT data_json FROM profiles WHERE tin = ?1",
                params![&draft.tin],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .ok_or_else(|| {
                DbError::Other(format!(
                    "Cannot queue 2551Q because taxpayer profile {} does not exist",
                    draft.tin
                ))
            })?;
        let mut profile: TaxpayerProfile = serde_json::from_str(&profile_json)?;
        let current_annual_election = annual_income_tax_election(&profile, draft.taxable_year);
        if current_annual_election == AnnualIncomeTaxElection::Conflicting {
            return Err(DbError::Other(format!(
                "Taxpayer profile records conflicting income-tax elections for {}",
                draft.taxable_year
            )));
        }

        let requests_eight_percent = draft.item_13_election == Item13Election::EightPercent;
        let requests_graduated = draft.item_13_election == Item13Election::Graduated;
        if requests_eight_percent && current_annual_election == AnnualIncomeTaxElection::Graduated {
            return Err(DbError::Other(format!(
                "Taxpayer profile already records a conflicting income-tax election for {}",
                draft.taxable_year
            )));
        }
        if requests_graduated && current_annual_election == AnnualIncomeTaxElection::EightPercent {
            return Err(DbError::Other(format!(
                "Taxpayer profile already records a conflicting income-tax election for {}",
                draft.taxable_year
            )));
        }

        let mut profile_changed = false;
        if (requests_eight_percent || requests_graduated)
            && current_annual_election == AnnualIncomeTaxElection::Unrecorded
        {
            // Keep the new election in this transaction's in-memory profile
            // until the fully synchronized queued draft validates. An invalid
            // or stale draft therefore cannot leave a partial ledger write.
            profile.tax_elections.push(TaxElectionHistory {
                taxable_year: draft.taxable_year,
                election: if requests_eight_percent {
                    IncomeTaxElection::EightPercent
                } else {
                    IncomeTaxElection::GraduatedUnspecified
                },
                elected_at: chrono::Utc::now().naive_utc(),
                source_form: "2551Qv2018".to_string(),
            });
            profile_changed = true;
        }

        let mut verified = draft.clone();
        verified
            .reconcile_with_effective_profile(&profile)
            .map_err(|error| {
                DbError::Other(format!(
                    "Cannot queue 2551Q until its exact filing period resolves to one confirmed taxpayer-profile version: {error}"
                ))
            })?;
        if let Err(validation_errors) = verified.revalidate_queued_before_submission() {
            let summary = validation_errors
                .iter()
                .map(|(field, message)| format!("{field}: {message}"))
                .collect::<Vec<_>>()
                .join("; ");
            return Err(DbError::Other(format!(
                "Queued 2551Q draft failed current-profile and queue-fingerprint validation: {summary}"
            )));
        }

        if profile_changed {
            let resolved = profile.resolve_tax_profile_for_year(draft.taxable_year);
            if resolved.has_blocking_issues() || resolved.effective_segments.is_empty() {
                let details = resolved
                    .issues
                    .iter()
                    .map(|issue| issue.message.as_str())
                    .collect::<Vec<_>>()
                    .join("; ");
                return Err(DbError::Other(format!(
                    "Cannot reconcile the {} Forms Set after recording Item 13 because the confirmed profile timeline is unresolved{}",
                    draft.taxable_year,
                    if details.is_empty() {
                        String::new()
                    } else {
                        format!(": {details}")
                    }
                )));
            }

            let stored_set =
                super::forms_set::query_per_year_forms(&tx, &draft.tin, draft.taxable_year)?;
            let existing_set = (!stored_set.is_empty()).then_some(&stored_set);
            let suggestions = crate::integration::validation::form_suggestions_for_profile_year(
                &profile,
                draft.taxable_year,
            );
            let reconciled = crate::forms::reconcile_forms_set_for_year(
                draft.taxable_year,
                existing_set,
                &suggestions,
            );
            if !reconciled.conflicts.is_empty() {
                tracing::warn!(
                    tin = %draft.tin,
                    taxable_year = draft.taxable_year,
                    conflicts = reconciled.conflicts.len(),
                    "Item 13 election reconciled a Forms Set that needs review"
                );
            }
            super::forms_set::execute_replace_per_year_forms(
                &tx,
                &draft.tin,
                draft.taxable_year,
                &reconciled.forms_set,
            )?;
            let updated_profile_json = serde_json::to_string(&profile)?;
            let updated = tx.execute(
                "UPDATE profiles SET data_json = ?1 WHERE tin = ?2",
                params![updated_profile_json, &draft.tin],
            )?;
            if updated != 1 {
                return Err(DbError::Other(format!(
                    "Expected one taxpayer profile for TIN {}, updated {updated}",
                    draft.tin
                )));
            }
        }

        let json = serde_json::to_string(&verified)?;
        let quarter = i64::from(verified.quarter);
        let period_key = FilingPeriod::Quarterly(verified.quarter).to_period_key();

        let id = if let Some((id, _, raw_json)) = existing {
            let updated = tx.execute(
                "UPDATE form_drafts
                 SET status = 'Queued', data_json = ?1, period_key = ?2,
                     updated_at = datetime('now')
                 WHERE id = ?3 AND status = 'Draft' AND data_json = ?4",
                params![json, &period_key, id, raw_json],
            )?;
            if updated != 1 {
                return Err(DbError::Other(
                    "2551Q draft changed before its queue snapshot was persisted".to_string(),
                ));
            }
            id
        } else {
            tx.execute(
                "INSERT INTO form_drafts
                    (tin, form_code, taxable_year, quarter, period_key, status, data_json)
                 VALUES (?1, '2551Q', ?2, ?3, ?4, 'Queued', ?5)",
                params![
                    &verified.tin,
                    i64::from(verified.taxable_year),
                    quarter,
                    &period_key,
                    json
                ],
            )?;
            tx.last_insert_rowid()
        };
        tx.commit()?;

        Ok(self.finish_post_commit_write(id, "Queued 2551Q election save"))
    }

    /// CAS-replace one exact, still-unclaimed 2551Q queue generation with
    /// either retry metadata or a deliberate return to Draft.
    pub(crate) fn replace_unclaimed_queued_2551q_submission(
        &self,
        replacement: &Form2551QDraft,
        expected_fingerprint: &Option<String>,
        expected_next_retry_at: &Option<String>,
        expected_submission_attempts: u32,
    ) -> Result<bool, DbError> {
        let mut replacement = replacement.clone();
        if !matches!(
            replacement.status,
            FilingStatus::Draft | FilingStatus::Queued
        ) {
            return Err(DbError::Other(
                "An unclaimed 2551Q queue generation may only remain Queued or return to Draft"
                    .to_string(),
            ));
        }
        if matches!(replacement.status, FilingStatus::Queued)
            && &replacement.queued_submission_fingerprint != expected_fingerprint
        {
            return Err(DbError::Other(
                "A retry update cannot change the reviewed 2551Q queue fingerprint".to_string(),
            ));
        }
        if matches!(replacement.status, FilingStatus::Queued) {
            if let Err(errors) = replacement.revalidate_queued_before_submission() {
                return Err(DbError::Other(format!(
                    "A retry update cannot persist invalid 2551Q submission fields: {}",
                    errors
                        .iter()
                        .map(|(field, message)| format!("{field}: {message}"))
                        .collect::<Vec<_>>()
                        .join("; ")
                )));
            }
        }
        if replacement.submission_claim_token.is_some()
            || replacement.submission_claimed_at.is_some()
        {
            return Err(DbError::Other(
                "An unclaimed 2551Q replacement cannot carry a network claim".to_string(),
            ));
        }

        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let Some((id, raw_json, db_status)) = tx
            .query_row(
                "SELECT id, data_json, status FROM form_drafts
                 WHERE tin = ?1 AND form_code = '2551Q'
                   AND taxable_year = ?2 AND quarter = ?3",
                params![
                    &replacement.tin,
                    i64::from(replacement.taxable_year),
                    i64::from(replacement.quarter)
                ],
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
            return Ok(false);
        };
        let current: Form2551QDraft = serde_json::from_str(&raw_json)?;
        if db_status != "Queued"
            || !matches!(current.status, FilingStatus::Queued)
            || current.submission_claim_token.is_some()
            || current.submission_claimed_at.is_some()
            || &current.queued_submission_fingerprint != expected_fingerprint
            || &current.next_retry_at != expected_next_retry_at
            || current.submission_attempts != expected_submission_attempts
        {
            return Ok(false);
        }

        let json = serde_json::to_string(&replacement)?;
        let status = filing_status_to_db(&replacement.status);
        let updated = tx.execute(
            "UPDATE form_drafts
             SET status = ?1, data_json = ?2, updated_at = datetime('now')
             WHERE id = ?3 AND status = 'Queued' AND data_json = ?4",
            params![status, json, id, raw_json],
        )?;
        if updated != 1 {
            return Ok(false);
        }
        tx.commit()?;
        let _ = self.request_post_commit_refresh();
        Ok(true)
    }

    /// Cancel one exact, still-unclaimed 2551Q queue generation.
    pub fn cancel_queued_2551q_submission(
        &self,
        queued: &Form2551QDraft,
    ) -> Result<Form2551QDraft, DbError> {
        if !matches!(queued.status, FilingStatus::Queued)
            || queued.submission_claim_token.is_some()
            || queued.submission_claimed_at.is_some()
        {
            return Err(DbError::Other(
                "Only an unclaimed queued 2551Q snapshot can be canceled".to_string(),
            ));
        }
        let expected_json = serde_json::to_string(queued)?;
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let Some((id, raw_json, db_status)) = tx
            .query_row(
                "SELECT id, data_json, status FROM form_drafts
                 WHERE tin = ?1 AND form_code = '2551Q'
                   AND taxable_year = ?2 AND quarter = ?3",
                params![
                    &queued.tin,
                    i64::from(queued.taxable_year),
                    i64::from(queued.quarter)
                ],
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
            return Err(DbError::Other(
                "2551Q submission has already started or the queue generation changed".to_string(),
            ));
        };
        let mut draft: Form2551QDraft = serde_json::from_str(&raw_json)?;
        if db_status != "Queued"
            || !matches!(draft.status, FilingStatus::Queued)
            || draft.submission_claim_token.is_some()
            || draft.submission_claimed_at.is_some()
            || raw_json != expected_json
        {
            return Err(DbError::Other(
                "2551Q submission has already started or the queue generation changed".to_string(),
            ));
        }

        draft.revert_to_draft();
        let json = serde_json::to_string(&draft)?;
        let updated = tx.execute(
            "UPDATE form_drafts
             SET status = 'Draft', data_json = ?1, updated_at = datetime('now')
             WHERE id = ?2 AND status = 'Queued' AND data_json = ?3",
            params![json, id, raw_json],
        )?;
        if updated != 1 {
            return Err(DbError::Other(
                "2551Q submission has already started or the queue generation changed".to_string(),
            ));
        }
        tx.commit()?;
        let _ = self.request_post_commit_refresh();
        Ok(draft)
    }

    /// Human-confirmed release of a claimed 2551Q queue snapshot back to Draft.
    /// Unclaimed queues use [`Self::cancel_queued_2551q_submission`]. Does not
    /// queue, transmit, or invent Submitted.
    pub fn release_abandoned_claimed_2551q_submission(
        &self,
        tin: &str,
        taxable_year: u16,
        quarter: u8,
        reason: &str,
    ) -> Result<AbandonedClaimRelease<Form2551QDraft>, DbError> {
        require_abandoned_release_reason(reason)?;
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let Some((id, raw_json, db_status)) = tx
            .query_row(
                "SELECT id, data_json, status FROM form_drafts
                 WHERE tin = ?1 AND form_code = '2551Q'
                   AND taxable_year = ?2 AND quarter = ?3",
                params![tin, i64::from(taxable_year), i64::from(quarter)],
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
            return Ok(AbandonedClaimRelease::AlreadyClear {
                previous_status: None,
                draft: None,
                reason: reason.to_string(),
            });
        };
        let current: Form2551QDraft = serde_json::from_str(&raw_json)?;
        let claimed = claim_fields_present(
            &current.submission_claim_token,
            &current.submission_claimed_at,
        );
        let partial = claim_fields_partial(
            &current.submission_claim_token,
            &current.submission_claimed_at,
        );
        if matches!(current.status, FilingStatus::Draft)
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
            current.status,
            FilingStatus::Sent | FilingStatus::Confirmed | FilingStatus::Paid
        ) || matches!(db_status.as_str(), "Submitted" | "Confirmed" | "Paid")
        {
            return Err(DbError::Other(
                "Will not release a 2551Q claim that is already Submitted, Confirmed, or Paid"
                    .to_string(),
            ));
        }
        if partial {
            return Err(DbError::Other(
                "2551Q claim metadata is incomplete; refusing to wipe an unknown filing state"
                    .to_string(),
            ));
        }
        if matches!(current.status, FilingStatus::Queued) && db_status == "Queued" && !claimed {
            return Err(DbError::Other(
                "Only a claimed queued 2551Q snapshot can use abandoned-claim release; unclaimed queues use form.revert_draft"
                    .to_string(),
            ));
        }
        if !(matches!(current.status, FilingStatus::Queued) && db_status == "Queued" && claimed) {
            return Err(DbError::Other(
                "2551Q is not an abandoned claimed queue snapshot".to_string(),
            ));
        }

        let previous_claimed_at = current.submission_claimed_at.clone();
        let mut draft = current;
        draft.revert_to_draft();
        draft.last_error = Some(abandoned_claim_audit(reason));
        let json = serde_json::to_string(&draft)?;
        let updated = tx.execute(
            "UPDATE form_drafts
             SET status = 'Draft', data_json = ?1, updated_at = datetime('now')
             WHERE id = ?2 AND status = 'Queued' AND data_json = ?3",
            params![json, id, raw_json],
        )?;
        if updated != 1 {
            return Err(DbError::Other(
                "2551Q claim changed before abandoned-claim release completed".to_string(),
            ));
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

    /// Atomically claim the exact queue generation that was revalidated by the
    /// background worker. Once claimed, generic draft writes (including a stale
    /// UI cancel/requeue) are rejected until the worker finishes the claim.
    ///
    /// After PUT is marked started, the claim is fail-closed (no lease retry).
    /// The worker claims after the SFTP session is open and immediately
    /// before PUT. A process crash after PUT start (or a PUT error) leaves an
    /// unknown network outcome: BIR may or may not have received the return.
    /// Automatically clearing or retrying that claim could file a duplicate
    /// return. Unstarted claims may recover after `CLAIM_LEASE` expires.
    /// Started or legacy (no-lease) claims stay fail-closed until a person
    /// reconciles them against the BIR confirmation or receipt.
    /// `release_abandoned_claimed_*` is the only deliberate human-confirmed path
    /// back to Draft for those rows. Connect/login/CWD failures happen before
    /// this transaction and stay unclaimed.
    #[allow(clippy::too_many_arguments)]
    pub fn claim_queued_2551q_submission(
        &self,
        tin: &str,
        taxable_year: u16,
        quarter: u8,
        expected_fingerprint: &Option<String>,
        expected_next_retry_at: &Option<String>,
        expected_submission_attempts: u32,
    ) -> Result<Claim2551QSubmissionResult, DbError> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let Some((id, raw_json, db_status)) = tx
            .query_row(
                "SELECT id, data_json, status FROM form_drafts
                 WHERE tin = ?1 AND form_code = '2551Q'
                   AND taxable_year = ?2 AND quarter = ?3",
                params![tin, i64::from(taxable_year), i64::from(quarter)],
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
            return Ok(Claim2551QSubmissionResult::Superseded);
        };
        let mut draft: Form2551QDraft = serde_json::from_str(&raw_json)?;
        if db_status != "Queued"
            || !matches!(draft.status, FilingStatus::Queued)
            || draft.submission_claim_token.is_some()
            || draft.submission_claimed_at.is_some()
            || &draft.queued_submission_fingerprint != expected_fingerprint
            || &draft.next_retry_at != expected_next_retry_at
            || draft.submission_attempts != expected_submission_attempts
        {
            return Ok(Claim2551QSubmissionResult::Superseded);
        }

        let profile_json = tx
            .query_row(
                "SELECT data_json FROM profiles WHERE tin = ?1",
                params![tin],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .ok_or_else(|| {
                DbError::Other(format!(
                    "Cannot claim 2551Q submission because taxpayer profile {tin} does not exist"
                ))
            })?;
        let profile: crate::profile::TaxpayerProfile = serde_json::from_str(&profile_json)?;
        let rejection_errors = match draft.reconcile_with_effective_profile(&profile) {
            Ok(()) => draft.revalidate_queued_before_submission().err(),
            Err(error) => {
                draft.revert_to_draft();
                draft.last_error = Some(format!(
                    "Submission blocked because the effective taxpayer profile is unresolved: {error}"
                ));
                Some(vec![("profile_resolution".to_string(), error)])
            }
        };
        if let Some(errors) = rejection_errors {
            let rejected_json = serde_json::to_string(&draft)?;
            let updated = tx.execute(
                "UPDATE form_drafts
                 SET status = 'Draft', data_json = ?1, updated_at = datetime('now')
                 WHERE id = ?2 AND status = 'Queued' AND data_json = ?3",
                params![rejected_json, id, raw_json],
            )?;
            if updated != 1 {
                return Ok(Claim2551QSubmissionResult::Superseded);
            }
            tx.commit()?;
            return Ok(Claim2551QSubmissionResult::Rejected { draft, errors });
        }

        let token = uuid::Uuid::new_v4().to_string();
        let claimed_at = chrono::Utc::now();
        draft.submission_claim_token = Some(token.clone());
        draft.submission_claimed_at = Some(claimed_at.to_rfc3339());
        draft.submission_claim_lease_until =
            Some(crate::filing_queue::claim_lease_until(claimed_at));
        draft.submission_put_started_at = None;
        draft.last_error = Some(
            "Submission outcome pending. Automatic retry is disabled unless the claim lease expires before PUT starts; keep any BIR confirmation or receipt and contact support for manual reconciliation before taking another submission action."
                .to_string(),
        );
        let claimed_json = serde_json::to_string(&draft)?;
        let updated = tx.execute(
            "UPDATE form_drafts
             SET data_json = ?1, updated_at = datetime('now')
             WHERE id = ?2 AND status = 'Queued' AND data_json = ?3",
            params![claimed_json, id, raw_json],
        )?;
        if updated != 1 {
            return Ok(Claim2551QSubmissionResult::Superseded);
        }
        tx.commit()?;
        Ok(Claim2551QSubmissionResult::Claimed { draft, token })
    }

    /// Finish a claimed network attempt as Submitted. Once a claim exists, any
    /// non-success STOR outcome is unknown and must leave the durable claim frozen
    /// for manual reconciliation. Retry/exhaustion transitions happen only
    /// before the network claim through the unclaimed queue-generation CAS.
    pub(crate) fn finish_claimed_2551q_submission(
        &self,
        draft: &Form2551QDraft,
        claim_token: &str,
    ) -> Result<i64, DbError> {
        if !matches!(draft.status, FilingStatus::Sent) {
            return Err(DbError::Other(
                "Only a Submitted 2551Q snapshot can finish a network claim".to_string(),
            ));
        }
        if draft.submission_claim_token.is_some() || draft.submission_claimed_at.is_some() {
            return Err(DbError::Other(
                "A finished 2551Q attempt must clear its own network claim metadata".to_string(),
            ));
        }

        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let Some((id, raw_json, db_status)) = tx
            .query_row(
                "SELECT id, data_json, status FROM form_drafts
                 WHERE tin = ?1 AND form_code = '2551Q'
                   AND taxable_year = ?2 AND quarter = ?3",
                params![
                    &draft.tin,
                    i64::from(draft.taxable_year),
                    i64::from(draft.quarter)
                ],
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
            return Err(DbError::Other(
                "Claimed 2551Q draft disappeared before the network attempt finished".to_string(),
            ));
        };
        let mut existing: Form2551QDraft = serde_json::from_str(&raw_json)?;
        if db_status != "Queued"
            || !matches!(existing.status, FilingStatus::Queued)
            || existing.submission_claim_token.as_deref() != Some(claim_token)
            || existing.submission_claimed_at.is_none()
        {
            return Err(DbError::Other(
                "2551Q submission claim no longer belongs to this worker".to_string(),
            ));
        }
        if draft.to_bir_field_map() != existing.to_bir_field_map() {
            return Err(DbError::Other(
                "Claimed 2551Q submission fields changed before completion".to_string(),
            ));
        }

        let expected_filename = existing.default_submission_filename();
        if draft.queued_submission_fingerprint != existing.queued_submission_fingerprint
            || draft.submission_filename.as_deref() != Some(expected_filename.as_str())
            || draft.submitted_at.is_none()
            || draft.submission_attempts != 0
            || draft.next_retry_at.is_some()
            || draft.last_error.is_some()
        {
            return Err(DbError::Other(
                "Claimed 2551Q submission completion did not preserve the reviewed fingerprint, IAF filename, timestamp, and terminal state"
                    .to_string(),
            ));
        }

        existing.status = FilingStatus::Sent;
        existing.submitted_at = draft.submitted_at.clone();
        if draft.submitted_payload.is_some() {
            existing.submitted_payload = draft.submitted_payload.clone();
        }
        existing.submission_filename = draft.submission_filename.clone();
        existing.submission_attempts = 0;
        existing.next_retry_at = None;
        existing.last_error = None;
        existing.submission_claim_token = None;
        existing.submission_claimed_at = None;
        existing.submission_claim_lease_until = None;
        existing.submission_put_started_at = None;
        existing.updated_at = chrono::Utc::now().to_rfc3339();
        let json = serde_json::to_string(&existing)?;
        let period_key = FilingPeriod::Quarterly(draft.quarter).to_period_key();
        let updated = tx.execute(
            "UPDATE form_drafts
             SET status = ?1, data_json = ?2, period_key = ?3,
                 updated_at = datetime('now')
             WHERE id = ?4 AND status = 'Queued' AND data_json = ?5",
            params!["Submitted", json, period_key, id, raw_json],
        )?;
        if updated != 1 {
            return Err(DbError::Other(
                "2551Q submission claim changed before completion".to_string(),
            ));
        }
        tx.commit()?;
        let _ = self.request_post_commit_refresh();
        Ok(id)
    }

    pub fn mark_claimed_2551q_put_started(
        &self,
        tin: &str,
        taxable_year: u16,
        quarter: u8,
        claim_token: &str,
    ) -> Result<bool, DbError> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let Some((id, raw_json)) = tx
            .query_row(
                "SELECT id, data_json FROM form_drafts
                 WHERE tin = ?1 AND form_code = '2551Q'
                   AND taxable_year = ?2 AND quarter = ?3 AND status = 'Queued'",
                params![tin, i64::from(taxable_year), i64::from(quarter)],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?
        else {
            return Ok(false);
        };
        let mut draft: Form2551QDraft = serde_json::from_str(&raw_json)?;
        if draft.submission_claim_token.as_deref() != Some(claim_token) {
            return Ok(false);
        }
        if draft.submission_put_started_at.is_none() {
            draft.submission_put_started_at = Some(chrono::Utc::now().to_rfc3339());
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

    pub fn recover_expired_unstarted_2551q_claim(
        &self,
        tin: &str,
        taxable_year: u16,
        quarter: u8,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool, DbError> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let Some((id, raw_json)) = tx
            .query_row(
                "SELECT id, data_json FROM form_drafts
                 WHERE tin = ?1 AND form_code = '2551Q'
                   AND taxable_year = ?2 AND quarter = ?3 AND status = 'Queued'",
                params![tin, i64::from(taxable_year), i64::from(quarter)],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?
        else {
            return Ok(false);
        };
        let mut draft: Form2551QDraft = serde_json::from_str(&raw_json)?;
        if !crate::filing_queue::claim_is_recoverable(
            draft.submission_claimed_at.as_deref(),
            draft.submission_claim_lease_until.as_deref(),
            draft.submission_put_started_at.as_deref(),
            now,
        ) {
            return Ok(false);
        }
        // Not a BIR-facing failure: release the claim without spending the
        // five-attempt budget or revoking the queue authorization.
        if !draft.release_expired_unstarted_claim(
            "Claim lease expired before PUT started; retrying the same authorized task".to_string(),
        ) {
            return Ok(false);
        }
        let json = serde_json::to_string(&draft)?;
        let updated = tx.execute(
            "UPDATE form_drafts
             SET status = 'Queued', data_json = ?1, updated_at = datetime('now')
             WHERE id = ?2 AND status = 'Queued' AND data_json = ?3",
            params![json, id, raw_json],
        )?;
        if updated != 1 {
            return Ok(false);
        }
        tx.commit()?;
        Ok(true)
    }

    /// CAS-advance one exact Submitted 2551Q snapshot to Confirmed.
    ///
    /// Receipt matching may add confirmation metadata, but it may not replace
    /// the transmitted filing fields or the queue fingerprint.
    pub fn save_confirmed_2551q_draft(&self, draft: &Form2551QDraft) -> Result<i64, DbError> {
        if !matches!(draft.status, FilingStatus::Confirmed)
            || draft.confirmed_at.is_none()
            || draft.submission_claim_token.is_some()
            || draft.submission_claimed_at.is_some()
        {
            return Err(DbError::Other(
                "Only a completed 2551Q confirmation may use the confirmation persistence path"
                    .to_string(),
            ));
        }

        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let Some((id, raw_json, db_status)) = tx
            .query_row(
                "SELECT id, data_json, status FROM form_drafts
                 WHERE tin = ?1 AND form_code = '2551Q'
                   AND taxable_year = ?2 AND quarter = ?3",
                params![
                    &draft.tin,
                    i64::from(draft.taxable_year),
                    i64::from(draft.quarter)
                ],
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
            return Err(DbError::Other(
                "Submitted 2551Q draft disappeared before confirmation".to_string(),
            ));
        };
        let mut existing: Form2551QDraft = serde_json::from_str(&raw_json)?;
        if db_status != "Submitted"
            || !matches!(existing.status, FilingStatus::Sent)
            || draft.to_bir_field_map() != existing.to_bir_field_map()
            || draft.queued_submission_fingerprint != existing.queued_submission_fingerprint
            || draft.submitted_at != existing.submitted_at
            || existing.submitted_at.is_none()
            || existing.submission_filename.is_none()
        {
            return Err(DbError::Other(
                "2551Q confirmation no longer matches the exact submitted snapshot".to_string(),
            ));
        }

        let confirmed_at = if let Some(receipt_id) = draft.receipt_id {
            let receipt = tx
                .query_row(
                    "SELECT filename, tin, form_type, period, received_date, received_time
                     FROM submission_receipts
                     WHERE id = ?1",
                    params![receipt_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, String>(5)?,
                        ))
                    },
                )
                .optional()?
                .ok_or_else(|| {
                    DbError::Other(format!(
                        "2551Q confirmation receipt {receipt_id} does not exist"
                    ))
                })?;
            let (
                receipt_filename,
                receipt_tin,
                receipt_form,
                receipt_period,
                received_date,
                received_time,
            ) = receipt;
            let submitted_filename = existing
                .submission_filename
                .as_deref()
                .expect("submitted filename checked above");
            let filename_identity = crate::receipt::split_bir_filename(&receipt_filename);
            let expected_period = existing.period_code();
            if receipt_tin != existing.tin
                || !super::receipts::is_audited_2551q_receipt_form_type(&receipt_form)
                || receipt_period != expected_period
                || !filename_identity.as_ref().is_some_and(
                    |(filename_tin, filename_form, filename_period)| {
                        filename_tin == &existing.tin
                            && filename_form == &receipt_form
                            && filename_period == &expected_period
                    },
                )
                || !matches_2551q_receipt_filename(submitted_filename, &receipt_filename)
                || !draft
                    .submission_filename
                    .as_deref()
                    .is_some_and(|filename| {
                        filename == receipt_filename || filename == submitted_filename
                    })
            {
                return Err(DbError::Other(
                    "2551Q confirmation receipt does not match the submitted taxpayer, audited form alias, period, and filename"
                        .to_string(),
                ));
            }

            let received_at = parse_2551q_receipt_timestamp(&received_date, &received_time)
                .ok_or_else(|| {
                    DbError::Other(
                        "2551Q confirmation receipt has an invalid received timestamp".to_string(),
                    )
                })?;
            let submitted_at = existing
                .submitted_at
                .as_deref()
                .expect("submitted timestamp checked above");
            chrono::DateTime::parse_from_rfc3339(submitted_at).map_err(|_| {
                DbError::Other(
                    "Submitted 2551Q snapshot has an invalid submission timestamp".to_string(),
                )
            })?;
            if !crate::filing_queue::receipt_belongs_to_this_generation(
                received_at,
                existing
                    .queue_authorization
                    .as_ref()
                    .map(|auth| auth.authorized_at.as_str()),
                submitted_at,
            ) {
                return Err(DbError::Other(
                    "2551Q confirmation receipt predates this queued generation".to_string(),
                ));
            }
            received_at.to_rfc3339()
        } else {
            if draft.submission_filename != existing.submission_filename {
                return Err(DbError::Other(
                    "Manual 2551Q confirmation cannot replace the submitted filename".to_string(),
                ));
            }
            let manual_confirmed_at = draft
                .confirmed_at
                .clone()
                .expect("confirmed timestamp checked above");
            let manual_confirmed_dt = chrono::DateTime::parse_from_rfc3339(&manual_confirmed_at)
                .map_err(|_| {
                    DbError::Other(
                        "Manual 2551Q confirmation has an invalid confirmation timestamp"
                            .to_string(),
                    )
                })?;
            let submitted_at = chrono::DateTime::parse_from_rfc3339(
                existing
                    .submitted_at
                    .as_deref()
                    .expect("submitted timestamp checked above"),
            )
            .map_err(|_| {
                DbError::Other(
                    "Submitted 2551Q snapshot has an invalid submission timestamp".to_string(),
                )
            })?;
            if manual_confirmed_dt < submitted_at {
                return Err(DbError::Other(
                    "Manual 2551Q confirmation predates the submitted snapshot".to_string(),
                ));
            }
            manual_confirmed_at
        };

        existing.status = FilingStatus::Confirmed;
        existing.confirmed_at = Some(confirmed_at);
        existing.receipt_id = draft.receipt_id;
        existing.updated_at = chrono::Utc::now().to_rfc3339();
        let json = serde_json::to_string(&existing)?;
        let updated = tx.execute(
            "UPDATE form_drafts
             SET status = 'Confirmed', data_json = ?1, updated_at = datetime('now')
             WHERE id = ?2 AND status = 'Submitted' AND data_json = ?3",
            params![json, id, raw_json],
        )?;
        if updated != 1 {
            return Err(DbError::Other(
                "2551Q submission changed before confirmation completed".to_string(),
            ));
        }
        tx.commit()?;
        let _ = self.request_post_commit_refresh();
        Ok(id)
    }

    /// CAS-advance one exact Confirmed 2551Q snapshot to Paid.
    pub fn save_paid_2551q_draft(&self, draft: &Form2551QDraft) -> Result<i64, DbError> {
        if !matches!(draft.status, FilingStatus::Paid)
            || draft.submission_claim_token.is_some()
            || draft.submission_claimed_at.is_some()
        {
            return Err(DbError::Other(
                "Only a Paid 2551Q return may use the payment persistence path".to_string(),
            ));
        }

        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let Some((id, raw_json, db_status)) = tx
            .query_row(
                "SELECT id, data_json, status FROM form_drafts
                 WHERE tin = ?1 AND form_code = '2551Q'
                   AND taxable_year = ?2 AND quarter = ?3",
                params![
                    &draft.tin,
                    i64::from(draft.taxable_year),
                    i64::from(draft.quarter)
                ],
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
            return Err(DbError::Other(
                "Confirmed 2551Q draft disappeared before payment completion".to_string(),
            ));
        };
        let mut existing: Form2551QDraft = serde_json::from_str(&raw_json)?;
        if db_status != "Confirmed"
            || !matches!(existing.status, FilingStatus::Confirmed)
            || draft.to_bir_field_map() != existing.to_bir_field_map()
            || draft.queued_submission_fingerprint != existing.queued_submission_fingerprint
            || draft.submitted_at != existing.submitted_at
            || draft.submission_filename != existing.submission_filename
            || draft.confirmed_at != existing.confirmed_at
            || draft.receipt_id != existing.receipt_id
        {
            return Err(DbError::Other(
                "2551Q payment no longer matches the exact confirmed snapshot".to_string(),
            ));
        }

        existing.status = FilingStatus::Paid;
        existing.updated_at = chrono::Utc::now().to_rfc3339();
        let json = serde_json::to_string(&existing)?;
        let updated = tx.execute(
            "UPDATE form_drafts
             SET status = 'Paid', data_json = ?1, updated_at = datetime('now')
             WHERE id = ?2 AND status = 'Confirmed' AND data_json = ?3",
            params![json, id, raw_json],
        )?;
        if updated != 1 {
            return Err(DbError::Other(
                "2551Q confirmation changed before payment completed".to_string(),
            ));
        }
        tx.commit()?;
        let _ = self.request_post_commit_refresh();
        Ok(id)
    }

    /// CAS-update only the local payment-receipt attachment path on an
    /// immutable Confirmed or Paid 2551Q snapshot.
    pub fn save_2551q_payment_receipt_path(&self, draft: &Form2551QDraft) -> Result<i64, DbError> {
        if !matches!(draft.status, FilingStatus::Confirmed | FilingStatus::Paid) {
            return Err(DbError::Other(
                "A 2551Q payment receipt may only be attached to a Confirmed or Paid return"
                    .to_string(),
            ));
        }

        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let Some((id, raw_json, db_status)) = tx
            .query_row(
                "SELECT id, data_json, status FROM form_drafts
                 WHERE tin = ?1 AND form_code = '2551Q'
                   AND taxable_year = ?2 AND quarter = ?3",
                params![
                    &draft.tin,
                    i64::from(draft.taxable_year),
                    i64::from(draft.quarter)
                ],
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
            return Err(DbError::Other(
                "2551Q return disappeared before its payment receipt was attached".to_string(),
            ));
        };
        let mut existing: Form2551QDraft = serde_json::from_str(&raw_json)?;
        if filing_status_from_db(&db_status) != existing.status
            || existing.status != draft.status
            || draft.to_bir_field_map() != existing.to_bir_field_map()
            || draft.queued_submission_fingerprint != existing.queued_submission_fingerprint
            || draft.submitted_at != existing.submitted_at
            || draft.submission_filename != existing.submission_filename
            || draft.confirmed_at != existing.confirmed_at
            || draft.receipt_id != existing.receipt_id
        {
            return Err(DbError::Other(
                "2551Q payment receipt no longer matches the exact immutable filing snapshot"
                    .to_string(),
            ));
        }

        existing.payment_receipt_path = draft.payment_receipt_path.clone();
        let json = serde_json::to_string(&existing)?;
        let updated = tx.execute(
            "UPDATE form_drafts
             SET data_json = ?1, updated_at = datetime('now')
             WHERE id = ?2 AND status = ?3 AND data_json = ?4",
            params![json, id, db_status, raw_json],
        )?;
        if updated != 1 {
            return Err(DbError::Other(
                "2551Q return changed before its payment receipt was attached".to_string(),
            ));
        }
        tx.commit()?;
        Ok(id)
    }

    /// Item 13 as this TIN's Q1 2551Q of `year` filed it, when this app
    /// holds that Q1 and it was filed (Submitted, Confirmed or Paid); `None`
    /// for no Q1 here, or one still a Draft or Queued.
    pub fn filed_2551q_q1_item_13(
        &self,
        tin: &str,
        year: u16,
    ) -> Result<Option<crate::forms::form_2551q::Item13Election>, DbError> {
        Ok(self.get_2551q_draft(tin, year, 1)?.and_then(|q1| {
            matches!(
                q1.status,
                FilingStatus::Sent | FilingStatus::Confirmed | FilingStatus::Paid
            )
            .then_some(q1.item_13_election)
        }))
    }

    /// Load a 2551Q draft for a specific (tin, year, quarter).
    /// Returns None if no draft exists for that slot.
    pub fn get_2551q_draft(
        &self,
        tin: &str,
        year: u16,
        quarter: u8,
    ) -> Result<Option<Form2551QDraft>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, data_json FROM form_drafts
             WHERE tin = ?1 AND form_code = '2551Q'
               AND taxable_year = ?2 AND quarter = ?3",
        )?;
        let mut rows = stmt.query(params![tin, year as i64, quarter as i64])?;
        if let Some(row) = rows.next()? {
            let id: i64 = row.get(0)?;
            let json: String = row.get(1)?;
            let mut draft: Form2551QDraft = serde_json::from_str(&json)?;
            draft.id = Some(id);
            Ok(Some(draft))
        } else {
            Ok(None)
        }
    }

    /// Save or update an editable Form 1601C draft.
    ///
    /// Generic saves may never create or replace Queued and later snapshots.
    /// Queue, cancellation, claim, and completion each use a dedicated CAS path.
    pub fn save_1601c_draft(&self, draft: &Form1601CDraft) -> Result<i64, DbError> {
        if !matches!(draft.status, FilingStatus::Draft) {
            return Err(DbError::Other(
                "Only an editable Draft 1601C return may use the generic save path".to_string(),
            ));
        }

        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let json = serde_json::to_string(draft)?;
        let month = i64::from(draft.month); // Legacy quarter column stores the month.
        let period_key = FilingPeriod::Monthly(draft.month).to_period_key();
        let existing = tx
            .query_row(
                "SELECT id, status, data_json FROM form_drafts
                 WHERE tin = ?1 AND form_code = '1601C'
                   AND taxable_year = ?2 AND quarter = ?3",
                params![&draft.tin, i64::from(draft.taxable_year), month],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?;

        let id = if let Some((id, db_status, raw_json)) = existing {
            if db_status != "Draft" {
                return Err(DbError::Other(
                    "An immutable queued or filed 1601C snapshot cannot be replaced by a generic draft save"
                        .to_string(),
                ));
            }
            let stored: Form1601CDraft = serde_json::from_str(&raw_json)?;
            if !matches!(stored.status, FilingStatus::Draft) {
                return Err(DbError::Other(
                    "An immutable queued or filed 1601C snapshot cannot be replaced by a generic draft save"
                        .to_string(),
                ));
            }
            let updated = tx.execute(
                "UPDATE form_drafts
                 SET data_json = ?1, period_key = ?2, updated_at = datetime('now')
                 WHERE id = ?3 AND status = 'Draft' AND data_json = ?4",
                params![json, period_key, id, raw_json],
            )?;
            if updated != 1 {
                return Err(DbError::Other(
                    "1601C draft changed before the editable save completed".to_string(),
                ));
            }
            id
        } else {
            tx.execute(
                "INSERT INTO form_drafts
                    (tin, form_code, taxable_year, quarter, period_key, status, data_json)
                 VALUES (?1, '1601C', ?2, ?3, ?4, 'Draft', ?5)",
                params![
                    &draft.tin,
                    i64::from(draft.taxable_year),
                    month,
                    period_key,
                    json
                ],
            )?;
            tx.last_insert_rowid()
        };

        tx.commit()?;
        let _ = self.request_post_commit_refresh();
        Ok(id)
    }

    /// Validate and atomically persist the exact user-reviewed queue snapshot.
    pub fn save_queued_1601c_draft(&self, draft: &Form1601CDraft) -> Result<i64, DbError> {
        if !matches!(draft.status, FilingStatus::Queued) {
            return Err(DbError::Other(
                "Only a queued 1601C draft can be saved through the submission path".to_string(),
            ));
        }
        if !draft.is_amended
            && self.has_filed_original(
                &draft.tin,
                "1601C",
                draft.taxable_year,
                &FilingPeriod::Monthly(draft.month).to_period_key(),
                draft.id,
            )?
        {
            return Err(DbError::AmendedReturnRequired);
        }

        let mut verified = draft.clone();
        if let Err(errors) = verified.revalidate_queued_before_submission() {
            let summary = errors
                .iter()
                .map(|(field, message)| format!("{field}: {message}"))
                .collect::<Vec<_>>()
                .join("; ");
            return Err(DbError::Other(format!(
                "Queued 1601C draft failed queue-fingerprint validation: {summary}"
            )));
        }
        verified.try_to_bir_xml_payload().map_err(|errors| {
            DbError::Other(format!(
                "Queued 1601C draft failed exact XML generation: {}",
                errors
                    .iter()
                    .map(|(field, message)| format!("{field}: {message}"))
                    .collect::<Vec<_>>()
                    .join("; ")
            ))
        })?;

        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let month = i64::from(verified.month);
        let period_key = FilingPeriod::Monthly(verified.month).to_period_key();
        let json = serde_json::to_string(&verified)?;
        let existing = tx
            .query_row(
                "SELECT id, status, data_json FROM form_drafts
                 WHERE tin = ?1 AND form_code = '1601C'
                   AND taxable_year = ?2 AND quarter = ?3",
                params![&verified.tin, i64::from(verified.taxable_year), month],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?;

        let id = if let Some((id, db_status, raw_json)) = existing {
            if db_status != "Draft" {
                return Err(DbError::Other(
                    "1601C must be canceled back to Draft before a new queue snapshot can replace it"
                        .to_string(),
                ));
            }
            let stored: Form1601CDraft = serde_json::from_str(&raw_json)?;
            if !matches!(stored.status, FilingStatus::Draft) {
                return Err(DbError::Other(
                    "1601C must be canceled back to Draft before a new queue snapshot can replace it"
                        .to_string(),
                ));
            }
            let updated = tx.execute(
                "UPDATE form_drafts
                 SET status = 'Queued', data_json = ?1, period_key = ?2,
                     updated_at = datetime('now')
                 WHERE id = ?3 AND status = 'Draft' AND data_json = ?4",
                params![json, period_key, id, raw_json],
            )?;
            if updated != 1 {
                return Err(DbError::Other(
                    "1601C draft changed before its queue snapshot was persisted".to_string(),
                ));
            }
            id
        } else {
            tx.execute(
                "INSERT INTO form_drafts
                    (tin, form_code, taxable_year, quarter, period_key, status, data_json)
                 VALUES (?1, '1601C', ?2, ?3, ?4, 'Queued', ?5)",
                params![
                    &verified.tin,
                    i64::from(verified.taxable_year),
                    month,
                    period_key,
                    json
                ],
            )?;
            tx.last_insert_rowid()
        };

        tx.commit()?;
        let _ = self.request_post_commit_refresh();
        Ok(id)
    }

    /// CAS-replace an unclaimed queue generation with a retry update or a
    /// deliberate Draft rejection/cancellation. The caller must identify the
    /// exact generation it previously loaded.
    pub(crate) fn replace_unclaimed_queued_1601c_submission(
        &self,
        replacement: &Form1601CDraft,
        expected_fingerprint: &Option<String>,
        expected_next_retry_at: &Option<String>,
        expected_submission_attempts: u32,
    ) -> Result<bool, DbError> {
        let mut replacement = replacement.clone();
        if !matches!(
            replacement.status,
            FilingStatus::Draft | FilingStatus::Queued
        ) {
            return Err(DbError::Other(
                "An unclaimed 1601C queue generation may only remain Queued or return to Draft"
                    .to_string(),
            ));
        }
        if matches!(replacement.status, FilingStatus::Queued)
            && &replacement.queued_submission_fingerprint != expected_fingerprint
        {
            return Err(DbError::Other(
                "A retry update cannot change the reviewed 1601C queue fingerprint".to_string(),
            ));
        }
        if matches!(replacement.status, FilingStatus::Queued) {
            if let Err(errors) = replacement.revalidate_queued_before_submission() {
                return Err(DbError::Other(format!(
                    "A retry update cannot persist invalid 1601C submission fields: {}",
                    errors
                        .iter()
                        .map(|(field, message)| format!("{field}: {message}"))
                        .collect::<Vec<_>>()
                        .join("; ")
                )));
            }
        }
        if replacement.submission_claim_token.is_some()
            || replacement.submission_claimed_at.is_some()
        {
            return Err(DbError::Other(
                "An unclaimed 1601C replacement cannot carry a network claim".to_string(),
            ));
        }

        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let Some((id, raw_json, db_status)) = tx
            .query_row(
                "SELECT id, data_json, status FROM form_drafts
                 WHERE tin = ?1 AND form_code = '1601C'
                   AND taxable_year = ?2 AND quarter = ?3",
                params![
                    &replacement.tin,
                    i64::from(replacement.taxable_year),
                    i64::from(replacement.month)
                ],
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
            return Ok(false);
        };
        let current: Form1601CDraft = serde_json::from_str(&raw_json)?;
        if db_status != "Queued"
            || !matches!(current.status, FilingStatus::Queued)
            || current.submission_claim_token.is_some()
            || current.submission_claimed_at.is_some()
            || &current.queued_submission_fingerprint != expected_fingerprint
            || &current.next_retry_at != expected_next_retry_at
            || current.submission_attempts != expected_submission_attempts
        {
            return Ok(false);
        }

        let json = serde_json::to_string(&replacement)?;
        let status = filing_status_to_db(&replacement.status);
        let updated = tx.execute(
            "UPDATE form_drafts
             SET status = ?1, data_json = ?2, updated_at = datetime('now')
             WHERE id = ?3 AND status = 'Queued' AND data_json = ?4",
            params![status, json, id, raw_json],
        )?;
        if updated != 1 {
            return Ok(false);
        }
        tx.commit()?;
        let _ = self.request_post_commit_refresh();
        Ok(true)
    }

    /// Cancel one exact, still-unclaimed 1601C queue generation.
    pub fn cancel_queued_1601c_submission(
        &self,
        queued: &Form1601CDraft,
    ) -> Result<Form1601CDraft, DbError> {
        if !matches!(queued.status, FilingStatus::Queued)
            || queued.submission_claim_token.is_some()
            || queued.submission_claimed_at.is_some()
        {
            return Err(DbError::Other(
                "Only an unclaimed queued 1601C snapshot can be canceled".to_string(),
            ));
        }
        let mut verified = queued.clone();
        if let Err(errors) = verified.revalidate_queued_before_submission() {
            return Err(DbError::Other(format!(
                "Only the exact reviewed 1601C queue snapshot can be canceled: {}",
                errors
                    .iter()
                    .map(|(field, message)| format!("{field}: {message}"))
                    .collect::<Vec<_>>()
                    .join("; ")
            )));
        }
        let expected_fingerprint = verified.queued_submission_fingerprint.clone();
        let expected_retry = verified.next_retry_at.clone();
        let expected_attempts = verified.submission_attempts;
        let mut draft = verified;
        draft.revert_to_draft();
        if !self.replace_unclaimed_queued_1601c_submission(
            &draft,
            &expected_fingerprint,
            &expected_retry,
            expected_attempts,
        )? {
            return Err(DbError::Other(
                "1601C submission has already started or the queue generation changed".to_string(),
            ));
        }
        Ok(draft)
    }

    /// Human-confirmed release of a claimed 1601C queue snapshot back to Draft.
    /// Unclaimed queues use [`Self::cancel_queued_1601c_submission`]. Does not
    /// queue, transmit, or invent Submitted.
    pub fn release_abandoned_claimed_1601c_submission(
        &self,
        tin: &str,
        taxable_year: u16,
        month: u8,
        reason: &str,
    ) -> Result<AbandonedClaimRelease<Form1601CDraft>, DbError> {
        require_abandoned_release_reason(reason)?;
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let Some((id, raw_json, db_status)) = tx
            .query_row(
                "SELECT id, data_json, status FROM form_drafts
                 WHERE tin = ?1 AND form_code = '1601C'
                   AND taxable_year = ?2 AND quarter = ?3",
                params![tin, i64::from(taxable_year), i64::from(month)],
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
            return Ok(AbandonedClaimRelease::AlreadyClear {
                previous_status: None,
                draft: None,
                reason: reason.to_string(),
            });
        };
        let current: Form1601CDraft = serde_json::from_str(&raw_json)?;
        let claimed = claim_fields_present(
            &current.submission_claim_token,
            &current.submission_claimed_at,
        );
        let partial = claim_fields_partial(
            &current.submission_claim_token,
            &current.submission_claimed_at,
        );
        if matches!(current.status, FilingStatus::Draft)
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
            current.status,
            FilingStatus::Sent | FilingStatus::Confirmed | FilingStatus::Paid
        ) || matches!(db_status.as_str(), "Submitted" | "Confirmed" | "Paid")
        {
            return Err(DbError::Other(
                "Will not release a 1601C claim that is already Submitted, Confirmed, or Paid"
                    .to_string(),
            ));
        }
        if partial {
            return Err(DbError::Other(
                "1601C claim metadata is incomplete; refusing to wipe an unknown filing state"
                    .to_string(),
            ));
        }
        if matches!(current.status, FilingStatus::Queued) && db_status == "Queued" && !claimed {
            return Err(DbError::Other(
                "Only a claimed queued 1601C snapshot can use abandoned-claim release; unclaimed queues use form.revert_draft"
                    .to_string(),
            ));
        }
        if !(matches!(current.status, FilingStatus::Queued) && db_status == "Queued" && claimed) {
            return Err(DbError::Other(
                "1601C is not an abandoned claimed queue snapshot".to_string(),
            ));
        }

        let previous_claimed_at = current.submission_claimed_at.clone();
        let mut draft = current;
        draft.revert_to_draft();
        draft.submission_error = Some(abandoned_claim_audit(reason));
        let json = serde_json::to_string(&draft)?;
        let updated = tx.execute(
            "UPDATE form_drafts
             SET status = 'Draft', data_json = ?1, updated_at = datetime('now')
             WHERE id = ?2 AND status = 'Queued' AND data_json = ?3",
            params![json, id, raw_json],
        )?;
        if updated != 1 {
            return Err(DbError::Other(
                "1601C claim changed before abandoned-claim release completed".to_string(),
            ));
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

    /// Atomically revalidate and claim the exact queued 1601C generation
    /// immediately before PUT. The worker opens the SFTP session first so a
    /// connect/login failure stays unclaimed.
    #[allow(clippy::too_many_arguments)]
    pub fn claim_queued_1601c_submission(
        &self,
        tin: &str,
        taxable_year: u16,
        month: u8,
        expected_fingerprint: &Option<String>,
        expected_next_retry_at: &Option<String>,
        expected_submission_attempts: u32,
    ) -> Result<Claim1601CSubmissionResult, DbError> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let Some((id, raw_json, db_status)) = tx
            .query_row(
                "SELECT id, data_json, status FROM form_drafts
                 WHERE tin = ?1 AND form_code = '1601C'
                   AND taxable_year = ?2 AND quarter = ?3",
                params![tin, i64::from(taxable_year), i64::from(month)],
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
            return Ok(Claim1601CSubmissionResult::Superseded);
        };
        let mut draft: Form1601CDraft = serde_json::from_str(&raw_json)?;
        if db_status != "Queued"
            || !matches!(draft.status, FilingStatus::Queued)
            || draft.submission_claim_token.is_some()
            || draft.submission_claimed_at.is_some()
            || &draft.queued_submission_fingerprint != expected_fingerprint
            || &draft.next_retry_at != expected_next_retry_at
            || draft.submission_attempts != expected_submission_attempts
        {
            return Ok(Claim1601CSubmissionResult::Superseded);
        }

        if let Err(errors) = draft.revalidate_queued_before_submission() {
            let rejected_json = serde_json::to_string(&draft)?;
            let updated = tx.execute(
                "UPDATE form_drafts
                 SET status = 'Draft', data_json = ?1, updated_at = datetime('now')
                 WHERE id = ?2 AND status = 'Queued' AND data_json = ?3",
                params![rejected_json, id, raw_json],
            )?;
            if updated != 1 {
                return Ok(Claim1601CSubmissionResult::Superseded);
            }
            tx.commit()?;
            return Ok(Claim1601CSubmissionResult::Rejected { draft, errors });
        }

        let token = uuid::Uuid::new_v4().to_string();
        let claimed_at = chrono::Utc::now();
        draft.submission_claim_token = Some(token.clone());
        draft.submission_claimed_at = Some(claimed_at.to_rfc3339());
        draft.submission_claim_lease_until =
            Some(crate::filing_queue::claim_lease_until(claimed_at));
        draft.submission_put_started_at = None;
        draft.submission_error = Some(
            "Submission outcome pending. Automatic retry is disabled unless the claim lease expires before PUT starts; keep any BIR confirmation or receipt and contact support for manual reconciliation before taking another submission action."
                .to_string(),
        );
        let claimed_json = serde_json::to_string(&draft)?;
        let updated = tx.execute(
            "UPDATE form_drafts
             SET data_json = ?1, updated_at = datetime('now')
             WHERE id = ?2 AND status = 'Queued' AND data_json = ?3",
            params![claimed_json, id, raw_json],
        )?;
        if updated != 1 {
            return Ok(Claim1601CSubmissionResult::Superseded);
        }
        tx.commit()?;
        Ok(Claim1601CSubmissionResult::Claimed { draft, token })
    }

    /// Finalize a claimed 1601C network attempt. Only the worker holding the
    /// durable token may replace the claimed snapshot.
    pub(crate) fn finish_claimed_1601c_submission(
        &self,
        draft: &Form1601CDraft,
        claim_token: &str,
    ) -> Result<i64, DbError> {
        if !matches!(draft.status, FilingStatus::Sent) {
            return Err(DbError::Other(
                "Only a Submitted 1601C snapshot can finish a network claim".to_string(),
            ));
        }
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let Some((id, raw_json, db_status)) = tx
            .query_row(
                "SELECT id, data_json, status FROM form_drafts
                 WHERE tin = ?1 AND form_code = '1601C'
                   AND taxable_year = ?2 AND quarter = ?3",
                params![
                    &draft.tin,
                    i64::from(draft.taxable_year),
                    i64::from(draft.month)
                ],
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
            return Err(DbError::Other(
                "Claimed 1601C draft disappeared before the network attempt finished".to_string(),
            ));
        };
        let existing: Form1601CDraft = serde_json::from_str(&raw_json)?;
        if db_status != "Queued"
            || !matches!(existing.status, FilingStatus::Queued)
            || existing.submission_claim_token.as_deref() != Some(claim_token)
            || existing.submission_claimed_at.is_none()
        {
            return Err(DbError::Other(
                "1601C submission claim no longer belongs to this worker".to_string(),
            ));
        }
        if draft.queued_submission_fingerprint != existing.queued_submission_fingerprint
            || draft.to_bir_field_map() != existing.to_bir_field_map()
        {
            return Err(DbError::Other(
                "Claimed 1601C submission fields changed before completion".to_string(),
            ));
        }
        let expected_filename = existing.default_submission_filename();
        if draft.submission_filename.as_deref() != Some(expected_filename.as_str())
            || draft.submitted_at.is_none()
        {
            return Err(DbError::Other(
                "Claimed 1601C submission completion did not preserve the reviewed IAF filename and timestamp"
                    .to_string(),
            ));
        }

        let mut finished = draft.clone();
        finished.submission_claim_token = None;
        finished.submission_claimed_at = None;
        finished.submission_claim_lease_until = None;
        finished.submission_put_started_at = None;
        let json = serde_json::to_string(&finished)?;
        let period_key = FilingPeriod::Monthly(finished.month).to_period_key();
        let updated = tx.execute(
            "UPDATE form_drafts
             SET status = 'Submitted', data_json = ?1, period_key = ?2,
                 updated_at = datetime('now')
             WHERE id = ?3 AND status = 'Queued' AND data_json = ?4",
            params![json, period_key, id, raw_json],
        )?;
        if updated != 1 {
            return Err(DbError::Other(
                "1601C submission claim changed before completion".to_string(),
            ));
        }
        tx.commit()?;
        let _ = self.request_post_commit_refresh();
        Ok(id)
    }

    /// Persist PUT-started on a live claim so a later crash cannot look like
    /// a pre-PUT lease expiry.
    pub fn mark_claimed_1601c_put_started(
        &self,
        tin: &str,
        taxable_year: u16,
        month: u8,
        claim_token: &str,
    ) -> Result<bool, DbError> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let Some((id, raw_json)) = tx
            .query_row(
                "SELECT id, data_json FROM form_drafts
                 WHERE tin = ?1 AND form_code = '1601C'
                   AND taxable_year = ?2 AND quarter = ?3 AND status = 'Queued'",
                params![tin, i64::from(taxable_year), i64::from(month)],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?
        else {
            return Ok(false);
        };
        let mut draft: Form1601CDraft = serde_json::from_str(&raw_json)?;
        if draft.submission_claim_token.as_deref() != Some(claim_token) {
            return Ok(false);
        }
        if draft.submission_put_started_at.is_none() {
            draft.submission_put_started_at = Some(chrono::Utc::now().to_rfc3339());
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

    /// Recover a claimed 1601C whose lease expired before PUT started.
    pub fn recover_expired_unstarted_1601c_claim(
        &self,
        tin: &str,
        taxable_year: u16,
        month: u8,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool, DbError> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let Some((id, raw_json)) = tx
            .query_row(
                "SELECT id, data_json FROM form_drafts
                 WHERE tin = ?1 AND form_code = '1601C'
                   AND taxable_year = ?2 AND quarter = ?3 AND status = 'Queued'",
                params![tin, i64::from(taxable_year), i64::from(month)],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?
        else {
            return Ok(false);
        };
        let mut draft: Form1601CDraft = serde_json::from_str(&raw_json)?;
        if !crate::filing_queue::claim_is_recoverable(
            draft.submission_claimed_at.as_deref(),
            draft.submission_claim_lease_until.as_deref(),
            draft.submission_put_started_at.as_deref(),
            now,
        ) {
            return Ok(false);
        }
        // Not a BIR-facing failure: release the claim without spending the
        // five-attempt budget or revoking the queue authorization.
        if !draft.release_expired_unstarted_claim(
            "Claim lease expired before PUT started; retrying the same authorized task".to_string(),
        ) {
            return Ok(false);
        }
        let json = serde_json::to_string(&draft)?;
        let updated = tx.execute(
            "UPDATE form_drafts
             SET status = 'Queued', data_json = ?1, updated_at = datetime('now')
             WHERE id = ?2 AND status = 'Queued' AND data_json = ?3",
            params![json, id, raw_json],
        )?;
        if updated != 1 {
            return Ok(false);
        }
        tx.commit()?;
        Ok(true)
    }

    /// CAS-advance one exact Submitted 1601C snapshot to Confirmed.
    pub fn save_confirmed_1601c_draft(&self, draft: &Form1601CDraft) -> Result<i64, DbError> {
        if !matches!(draft.status, FilingStatus::Confirmed)
            || draft.confirmed_at.is_none()
            || draft.submission_claim_token.is_some()
            || draft.submission_claimed_at.is_some()
        {
            return Err(DbError::Other(
                "Only a completed 1601C confirmation may use the confirmation persistence path"
                    .to_string(),
            ));
        }

        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let Some((id, raw_json, db_status)) = tx
            .query_row(
                "SELECT id, data_json, status FROM form_drafts
                 WHERE tin = ?1 AND form_code = '1601C'
                   AND taxable_year = ?2 AND quarter = ?3",
                params![
                    &draft.tin,
                    i64::from(draft.taxable_year),
                    i64::from(draft.month)
                ],
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
            return Err(DbError::Other(
                "Submitted 1601C draft disappeared before confirmation".to_string(),
            ));
        };
        let mut existing: Form1601CDraft = serde_json::from_str(&raw_json)?;
        if db_status != "Submitted"
            || !matches!(existing.status, FilingStatus::Sent)
            || draft.to_bir_field_map() != existing.to_bir_field_map()
            || draft.queued_submission_fingerprint != existing.queued_submission_fingerprint
            || draft.submitted_at != existing.submitted_at
            || existing.submitted_at.is_none()
            || existing.submission_filename.is_none()
        {
            return Err(DbError::Other(
                "1601C confirmation no longer matches the exact submitted snapshot".to_string(),
            ));
        }

        let confirmed_at = if let Some(receipt_id) = draft.receipt_id {
            let receipt = tx
                .query_row(
                    "SELECT filename, tin, form_type, period, received_date, received_time
                     FROM submission_receipts
                     WHERE id = ?1",
                    params![receipt_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, String>(5)?,
                        ))
                    },
                )
                .optional()?
                .ok_or_else(|| {
                    DbError::Other(format!(
                        "1601C confirmation receipt {receipt_id} does not exist"
                    ))
                })?;
            let (
                receipt_filename,
                receipt_tin,
                receipt_form,
                receipt_period,
                received_date,
                received_time,
            ) = receipt;
            let expected_period = existing.period_code();
            if receipt_tin != existing.tin
                || !crate::filing_queue::is_audited_1601c_receipt_form_type(&receipt_form)
                || receipt_period != expected_period
            {
                return Err(DbError::Other(
                    "1601C confirmation receipt does not match the submitted taxpayer, form, and period"
                        .to_string(),
                ));
            }
            let submitted_filename = existing
                .submission_filename
                .as_deref()
                .expect("submitted filename checked above");
            let receipt_identity = crate::receipt::split_bir_filename(&receipt_filename);
            let submitted_identity = crate::receipt::split_bir_filename(submitted_filename);
            if receipt_identity != submitted_identity {
                return Err(DbError::Other(
                    "1601C confirmation receipt filename does not match the submitted IAF"
                        .to_string(),
                ));
            }

            let received_at = parse_2551q_receipt_timestamp(&received_date, &received_time)
                .ok_or_else(|| {
                    DbError::Other(
                        "1601C confirmation receipt has an invalid received timestamp".to_string(),
                    )
                })?;
            let submitted_at = existing
                .submitted_at
                .as_deref()
                .expect("submitted timestamp checked above");
            chrono::DateTime::parse_from_rfc3339(submitted_at).map_err(|_| {
                DbError::Other(
                    "Submitted 1601C snapshot has an invalid submission timestamp".to_string(),
                )
            })?;
            if !crate::filing_queue::receipt_belongs_to_this_generation(
                received_at,
                existing
                    .queue_authorization
                    .as_ref()
                    .map(|auth| auth.authorized_at.as_str()),
                submitted_at,
            ) {
                return Err(DbError::Other(
                    "1601C confirmation receipt predates this queued generation".to_string(),
                ));
            }
            received_at.to_rfc3339()
        } else {
            if draft.submission_filename != existing.submission_filename {
                return Err(DbError::Other(
                    "Manual 1601C confirmation cannot replace the submitted filename".to_string(),
                ));
            }
            let manual_confirmed_at = draft
                .confirmed_at
                .clone()
                .expect("confirmed timestamp checked above");
            let manual_confirmed_dt = chrono::DateTime::parse_from_rfc3339(&manual_confirmed_at)
                .map_err(|_| {
                    DbError::Other(
                        "Manual 1601C confirmation has an invalid confirmation timestamp"
                            .to_string(),
                    )
                })?;
            let submitted_at = chrono::DateTime::parse_from_rfc3339(
                existing
                    .submitted_at
                    .as_deref()
                    .expect("submitted timestamp checked above"),
            )
            .map_err(|_| {
                DbError::Other(
                    "Submitted 1601C snapshot has an invalid submission timestamp".to_string(),
                )
            })?;
            if manual_confirmed_dt < submitted_at {
                return Err(DbError::Other(
                    "Manual 1601C confirmation predates the submitted snapshot".to_string(),
                ));
            }
            manual_confirmed_at
        };

        existing.status = FilingStatus::Confirmed;
        existing.confirmed_at = Some(confirmed_at);
        existing.receipt_id = draft.receipt_id;
        existing.updated_at = chrono::Utc::now().to_rfc3339();
        let json = serde_json::to_string(&existing)?;
        let updated = tx.execute(
            "UPDATE form_drafts
             SET status = 'Confirmed', data_json = ?1, updated_at = datetime('now')
             WHERE id = ?2 AND status = 'Submitted' AND data_json = ?3",
            params![json, id, raw_json],
        )?;
        if updated != 1 {
            return Err(DbError::Other(
                "1601C submission changed before confirmation completed".to_string(),
            ));
        }
        tx.commit()?;
        let _ = self.request_post_commit_refresh();
        Ok(id)
    }

    /// Load a 1601C draft for a specific (tin, year, month).
    pub fn get_1601c_draft(
        &self,
        tin: &str,
        year: u16,
        month: u8,
    ) -> Result<Option<Form1601CDraft>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, data_json FROM form_drafts
             WHERE tin = ?1 AND form_code = '1601C'
               AND taxable_year = ?2 AND quarter = ?3",
        )?;
        let mut rows = stmt.query(params![tin, year as i64, month as i64])?;
        if let Some(row) = rows.next()? {
            let id: i64 = row.get(0)?;
            let json: String = row.get(1)?;
            let mut draft: Form1601CDraft = serde_json::from_str(&json)?;
            draft.id = Some(id);
            Ok(Some(draft))
        } else {
            Ok(None)
        }
    }

    /// Save an imported form directly to the form_drafts table to show up in Dashboard.
    pub fn save_imported_form(
        &self,
        tin: &str,
        form_code: &str,
        year: u16,
        quarter: Option<u8>,
        month: Option<u8>,
    ) -> Result<i64, DbError> {
        if form_code == "2551Q" {
            return Err(DbError::Other(
                "Imported 2551Q returns cannot bypass the audited draft, queue, claim, and confirmation persistence paths"
                    .to_string(),
            ));
        }

        let legacy_slot = match frequency_for_form(form_code) {
            FilingFrequency::Monthly => month.or(quarter),
            FilingFrequency::Quarterly => quarter.or(month),
            FilingFrequency::Annual => None,
            FilingFrequency::OpenEnded => quarter.or(month),
        };
        let default_open_ended_key =
            if matches!(frequency_for_form(form_code), FilingFrequency::OpenEnded)
                && legacy_slot.is_none_or(|value| value == 0)
            {
                self.next_open_ended_period_number(tin, form_code, year)?
            } else {
                1
            };
        let period = period_from_legacy_slot(form_code, legacy_slot, default_open_ended_key);
        let legacy_slot = legacy_slot_for_period(&period);
        let period_key = period.to_period_key();

        if form_code == "1601C" {
            let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
            let existing = tx
                .query_row(
                    "SELECT id, status, data_json FROM form_drafts
                     WHERE tin = ?1 AND form_code = '1601C'
                       AND taxable_year = ?2 AND period_key = ?3",
                    params![tin, i64::from(year), &period_key],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )
                .optional()?;

            let id = if let Some((id, status, raw_json)) = existing {
                if status != "Draft" {
                    return Err(DbError::Other(
                        "An imported 1601C return cannot replace a queued or filed snapshot"
                            .to_string(),
                    ));
                }
                let stored: Form1601CDraft = serde_json::from_str(&raw_json)?;
                if !matches!(stored.status, FilingStatus::Draft) {
                    return Err(DbError::Other(
                        "An imported 1601C return cannot replace a queued or filed snapshot"
                            .to_string(),
                    ));
                }
                let updated = tx.execute(
                    "UPDATE form_drafts
                     SET quarter = ?1, status = 'Submitted', data_json = '{}',
                         updated_at = datetime('now')
                     WHERE id = ?2 AND status = 'Draft' AND data_json = ?3",
                    params![legacy_slot, id, raw_json],
                )?;
                if updated != 1 {
                    return Err(DbError::Other(
                        "1601C draft changed before the imported return was saved".to_string(),
                    ));
                }
                id
            } else {
                tx.execute(
                    "INSERT INTO form_drafts
                        (tin, form_code, taxable_year, quarter, period_key, status, data_json)
                     VALUES (?1, '1601C', ?2, ?3, ?4, 'Submitted', '{}')",
                    params![tin, i64::from(year), legacy_slot, &period_key],
                )?;
                tx.last_insert_rowid()
            };
            tx.commit()?;
            let _ = self.request_post_commit_refresh();
            return Ok(id);
        }

        let rows_updated = self.conn.execute(
            "UPDATE form_drafts
             SET quarter = ?4,
                 status = 'Submitted',
                 data_json = '{}',
                 updated_at = datetime('now')
             WHERE tin = ?1
               AND form_code = ?2
               AND taxable_year = ?3
               AND period_key = ?5",
            params![tin, form_code, year as i64, legacy_slot, &period_key],
        )?;

        if rows_updated == 0 {
            self.conn.execute(
                "INSERT INTO form_drafts (tin, form_code, taxable_year, quarter, period_key, status, data_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'Submitted', '{}')",
                params![tin, form_code, year as i64, legacy_slot, &period_key],
            )?;
        }

        let id = self.conn.query_row(
            "SELECT id FROM form_drafts
             WHERE tin = ?1 AND form_code = ?2 AND taxable_year = ?3 AND period_key = ?4
             ORDER BY updated_at DESC, id DESC
             LIMIT 1",
            params![tin, form_code, year as i64, &period_key],
            |row| row.get::<_, i64>(0),
        )?;
        let _ = self.request_post_commit_refresh();
        Ok(id)
    }

    /// Get filing progress for a form in a given year.
    /// Returns a FormFilingProgress with per-quarter states.
    pub fn get_form_filing_progress(
        &self,
        tin: &str,
        form_code: &str,
        year: u16,
    ) -> Result<FormFilingProgress, DbError> {
        let mut progress = FormFilingProgress::new_empty(form_code, year);
        let frequency = frequency_for_form(form_code);
        let monthly = matches!(frequency, FilingFrequency::Monthly);

        let mut stmt = self.conn.prepare(
            "SELECT quarter, period_key, status FROM form_drafts
             WHERE tin = ?1 AND form_code = ?2 AND taxable_year = ?3",
        )?;
        let rows = stmt.query_map(params![tin, form_code, year as i64], |row| {
            Ok((
                row.get::<_, Option<i64>>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;

        for row in rows {
            let (legacy_slot, period_key, status_str) = row?;
            let state = quarter_state_from_db(&status_str);
            let period = period_from_row(form_code, legacy_slot, period_key.as_deref());
            apply_period_state(&mut progress, monthly, period, state);
        }

        // A Final Copy migrated from the old eBIRForms app counts as filed.
        let legacy_tin = super::legacy_filed_returns::canonical_legacy_tin(tin);
        let mut legacy_stmt = self.conn.prepare(
            "SELECT form_id, form_identity, period FROM legacy_filed_returns WHERE tin = ?1",
        )?;
        let legacy_rows = legacy_stmt.query_map(params![legacy_tin], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let mut legacy_open_ended = std::collections::BTreeSet::new();
        for row in legacy_rows {
            let (form_id, form_identity, period) = row?;
            let capability = form_id
                .as_deref()
                .and_then(crate::official_import::find_capability_identity)
                .or_else(|| crate::official_import::find_capability_identity(&form_identity));
            let Some(capability) = capability else {
                continue;
            };
            if capability.code != form_code {
                continue;
            }
            match legacy_return_period(form_code, &period, year) {
                Some(FilingPeriod::OpenEnded(_))
                    if matches!(frequency, FilingFrequency::OpenEnded) =>
                {
                    // An open-ended form has no period tiles; each distinct
                    // filed period of the year counts once (amendments and
                    // same-period copies dedupe by period).
                    legacy_open_ended.insert(period);
                }
                Some(period) => {
                    apply_period_state(&mut progress, monthly, period, QuarterState::Sent);
                }
                None => {}
            }
        }
        progress.open_ended_count = progress
            .open_ended_count
            .saturating_add(legacy_open_ended.len() as u32);

        Ok(progress)
    }

    /// List all form draft summaries for a TIN in a given year (all form types).
    pub fn list_draft_summaries(
        &self,
        tin: &str,
        year: u16,
    ) -> Result<Vec<FormDraftSummary>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, tin, form_code, taxable_year, quarter, period_key, status, updated_at
             FROM form_drafts WHERE tin = ?1 AND taxable_year = ?2",
        )?;
        let rows = stmt.query_map(params![tin, year as i64], draft_summary_from_row)?;

        let mut summaries = Vec::new();
        for row in rows {
            summaries.push(row?);
        }
        Ok(summaries)
    }

    /// Every form draft summary for a TIN, whatever its taxable year.
    pub fn list_draft_summaries_all_years(
        &self,
        tin: &str,
    ) -> Result<Vec<FormDraftSummary>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, tin, form_code, taxable_year, quarter, period_key, status, updated_at
             FROM form_drafts WHERE tin = ?1",
        )?;
        let rows = stmt.query_map(params![tin], draft_summary_from_row)?;
        let mut summaries = Vec::new();
        for row in rows {
            summaries.push(row?);
        }
        Ok(summaries)
    }

    /// Every stored return of `form_code` for `tin`, newest first, each with
    /// its stored key (the period column [`Self::draft_period_column`]
    /// reads: a month, a quarter, a transaction's `MMDD`, an entry number).
    pub fn list_form_returns(
        &self,
        tin: &str,
        form_code: &str,
    ) -> Result<Vec<(FormDraftSummary, Option<i64>)>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, tin, form_code, taxable_year, quarter, period_key, status, updated_at
             FROM form_drafts WHERE tin = ?1 AND form_code = ?2
             ORDER BY updated_at DESC, id DESC",
        )?;
        let rows = stmt.query_map(params![tin, form_code], |row| {
            Ok((draft_summary_from_row(row)?, row.get::<_, Option<i64>>(4)?))
        })?;
        let mut returns = Vec::new();
        for row in rows {
            returns.push(row?);
        }
        Ok(returns)
    }

    /// The returns of [`Self::list_form_returns`] not yet submitted: still
    /// Draft, never queued or filed. Newest first.
    pub fn unsubmitted_drafts(
        &self,
        tin: &str,
        form_code: &str,
    ) -> Result<Vec<(FormDraftSummary, Option<i64>)>, DbError> {
        Ok(self
            .list_form_returns(tin, form_code)?
            .into_iter()
            .filter(|(summary, _)| summary.status == FilingStatus::Draft)
            .collect())
    }

    /// The stored key of the saved return `draft_id`: its period column (a
    /// month, a quarter, a transaction's `MMDD`), what opens exactly that
    /// return. `None` when the row is gone or keeps no column.
    pub fn draft_period_column(&self, draft_id: i64) -> Result<Option<i64>, DbError> {
        Ok(self
            .conn
            .query_row(
                "SELECT quarter FROM form_drafts WHERE id = ?1",
                params![draft_id],
                |row| row.get::<_, Option<i64>>(0),
            )
            .optional()?
            .flatten())
    }

    /// Delete the saved return `draft_id` while it is still a Draft; a
    /// queued or filed return is kept. Whether a row was deleted.
    pub fn delete_draft(&self, draft_id: i64) -> Result<bool, DbError> {
        Ok(self.conn.execute(
            "DELETE FROM form_drafts WHERE id = ?1 AND status = 'Draft'",
            params![draft_id],
        )? > 0)
    }

    /// The stored JSON of the saved return `draft_id`, whatever its form:
    /// what reads a return's filing marks (`submitted_at`, `confirmed_at`,
    /// `submission_filename`) without its type. `None` when the row is gone
    /// or its JSON does not parse.
    pub fn draft_data_json(&self, draft_id: i64) -> Result<Option<serde_json::Value>, DbError> {
        Ok(self
            .conn
            .query_row(
                "SELECT data_json FROM form_drafts WHERE id = ?1",
                params![draft_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .and_then(|json| serde_json::from_str(&json).ok()))
    }

    /// The trigger date the saved as-needed return `draft_id` holds and its
    /// due-date rule (see [`crate::forms::event_trigger::event_trigger_from_json`]).
    /// `None` when the row is gone, the form is not as needed, or the return
    /// does not hold the date yet.
    pub fn draft_event_trigger(
        &self,
        draft_id: i64,
    ) -> Result<Option<crate::forms::event_trigger::EventTrigger>, DbError> {
        let row = self
            .conn
            .query_row(
                "SELECT form_code, data_json FROM form_drafts WHERE id = ?1",
                params![draft_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        Ok(row.and_then(|(form_code, data_json)| {
            crate::forms::event_trigger::event_trigger_from_json(&form_code, &data_json)
        }))
    }

    /// The total amount payable the saved return `draft_id` computed for
    /// itself (see [`crate::forms::calendar_status::amount_payable_from_json`]).
    /// `None` when the row is gone or its form has no readable total.
    pub fn draft_amount_payable(&self, draft_id: i64) -> Result<Option<f64>, DbError> {
        let row = self
            .conn
            .query_row(
                "SELECT form_code, data_json FROM form_drafts WHERE id = ?1",
                params![draft_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        Ok(row.and_then(|(form_code, data_json)| {
            crate::forms::calendar_status::amount_payable_from_json(&form_code, &data_json)
        }))
    }

    /// The total penalties a saved, unfiled return would carry if filed on
    /// `filing_date` (see [`crate::forms::calendar_status::penalties_payable_from_json`]).
    pub fn draft_penalties_if_filed_on(
        &self,
        draft_id: i64,
        profile: &crate::profile::TaxpayerProfile,
        deadline_overrides: &[crate::calendar_rules::DeadlineOverride],
        filing_date: chrono::NaiveDate,
    ) -> Result<Option<f64>, DbError> {
        let row = self
            .conn
            .query_row(
                "SELECT form_code, data_json FROM form_drafts WHERE id = ?1",
                params![draft_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        Ok(row.and_then(|(form_code, data_json)| {
            crate::forms::calendar_status::penalties_payable_from_json(
                &form_code,
                &data_json,
                profile,
                deadline_overrides,
                filing_date,
            )
        }))
    }

    pub fn list_all_queued_submissions(&self) -> Result<Vec<FormDraftSummary>, DbError> {
        self.list_form_drafts_with_status(&["Queued", "Submitted"])
    }

    /// Every return that has left Draft: Queued, Submitted, Confirmed, Paid.
    ///
    /// `list_all_queued_submissions` stops at Submitted because the cron and
    /// the Background Tasks cards only care about work still in flight. An
    /// agent tracking a filing through `submissions.list` needs to keep seeing
    /// it after BIR's receipt lands, so this one carries the settled states too.
    pub fn list_all_filed_submissions(&self) -> Result<Vec<FormDraftSummary>, DbError> {
        self.list_form_drafts_with_status(&["Queued", "Submitted", "Confirmed", "Paid"])
    }

    fn list_form_drafts_with_status(
        &self,
        statuses: &[&str],
    ) -> Result<Vec<FormDraftSummary>, DbError> {
        // Fixed literals from the two callers above, never caller input.
        let placeholders = statuses.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
        let mut stmt = self.conn.prepare(&format!(
            "SELECT id, tin, form_code, taxable_year, quarter, period_key, status, updated_at
             FROM form_drafts
             WHERE status IN ({placeholders})"
        ))?;
        let rows = stmt.query_map(rusqlite::params_from_iter(statuses.iter()), |row| {
            let form_code: String = row.get(2)?;
            let period_val = row.get::<_, Option<i64>>(4)?.map(|q| q as u8);
            let legacy_slot = row.get::<_, Option<i64>>(4)?;
            let period_key = row.get::<_, Option<String>>(5)?;
            let period = period_from_row(&form_code, legacy_slot, period_key.as_deref());
            let (quarter, month) = summary_period_fields(&period);
            let frequency = frequency_for_form(&form_code);
            let quarter = quarter.or({
                if matches!(&frequency, FilingFrequency::Quarterly) {
                    period_val
                } else {
                    None
                }
            });
            let month = month.or({
                if matches!(&frequency, FilingFrequency::Monthly) {
                    period_val
                } else {
                    None
                }
            });

            Ok(FormDraftSummary {
                id: row.get(0)?,
                tin: row.get(1)?,
                form_code,
                taxable_year: row.get::<_, i64>(3)? as u16,
                quarter,
                month,
                status: filing_status_from_db(row.get::<_, String>(6)?.as_str()),
                updated_at: row.get(7)?,
            })
        })?;

        let mut summaries = Vec::new();
        for row in rows {
            let summary = row?;
            if crate::forms::can_queue_for_submission(&summary.form_code) {
                summaries.push(summary);
            }
        }
        Ok(summaries)
    }

    // ── Period-key-aware methods (v2) ──
    //
    // These use the `period_key` column added in v5 migration.
    // They complement the legacy methods above which use the raw `quarter` column.

    /// Save or update a form draft using a `period_key` for unified period handling.
    ///
    /// Updates by (tin, form_code, taxable_year, period_key), then inserts if absent.
    /// This avoids SQLite's nullable `quarter` uniqueness edge case for annual/open-ended forms.
    pub fn save_form_draft_v2<T: serde::Serialize>(
        &self,
        tin: &str,
        form_code: &str,
        year: u16,
        period: &crate::forms::FilingPeriod,
        status: &FilingStatus,
        draft: &T,
    ) -> Result<i64, DbError> {
        if matches!(form_code, "1601C" | "2551Q") {
            return Err(DbError::Other(format!(
                "{form_code} must use its dedicated immutable draft or queue persistence path"
            )));
        }

        if matches!(status, FilingStatus::Queued) {
            return Err(DbError::Other(format!(
                "Form {form_code} cannot enter Queued state through generic draft persistence; use its dedicated reviewed queue path"
            )));
        }

        let json = serde_json::to_string(draft)?;
        let status_str = filing_status_to_db(status);
        let period_key = period.to_period_key();

        // Also set the legacy quarter column for backward compatibility
        let quarter_val = legacy_slot_for_period(period);

        // Only a row still in Draft is ever replaced: a queued, submitted,
        // confirmed or paid return is the filed record, and saving over it
        // would lose what reached BIR. A save then fails instead of writing.
        let mut stored = self.conn.prepare(
            "SELECT status FROM form_drafts
             WHERE tin = ?1 AND form_code = ?2 AND taxable_year = ?3 AND period_key = ?4",
        )?;
        let left_draft = stored
            .query_map(params![tin, form_code, year as i64, &period_key], |row| {
                row.get::<_, String>(0)
            })?
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(|status| filing_status_from_db(&status))
            .find(|status| *status != FilingStatus::Draft);
        drop(stored);
        if let Some(status) = left_draft {
            return Err(DbError::Other(format!(
                "The stored {form_code} return for this period is {}, no longer a draft; it cannot be saved over. Amend it instead.",
                filing_status_to_db(&status).to_lowercase()
            )));
        }

        let rows_updated = self.conn.execute(
            "UPDATE form_drafts
             SET quarter = ?4,
                 status = ?5,
                 data_json = ?6,
                 updated_at = datetime('now')
             WHERE tin = ?1
               AND form_code = ?2
               AND taxable_year = ?3
               AND period_key = ?7",
            params![
                tin,
                form_code,
                year as i64,
                quarter_val,
                status_str,
                json,
                &period_key
            ],
        )?;

        if rows_updated == 0 {
            self.conn.execute(
                "INSERT INTO form_drafts (tin, form_code, taxable_year, quarter, period_key, status, data_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    tin,
                    form_code,
                    year as i64,
                    quarter_val,
                    &period_key,
                    status_str,
                    json
                ],
            )?;
        }

        let id = self.conn.query_row(
            "SELECT id FROM form_drafts
             WHERE tin = ?1 AND form_code = ?2 AND taxable_year = ?3 AND period_key = ?4
             ORDER BY updated_at DESC, id DESC
             LIMIT 1",
            params![tin, form_code, year as i64, &period_key],
            |row| row.get::<_, i64>(0),
        )?;
        Ok(id)
    }

    /// Load a form draft by period_key.
    pub fn get_form_draft_v2<T: serde::de::DeserializeOwned>(
        &self,
        tin: &str,
        form_code: &str,
        year: u16,
        period: &crate::forms::FilingPeriod,
    ) -> Result<Option<T>, DbError> {
        let period_key = period.to_period_key();
        let mut stmt = self.conn.prepare(
            "SELECT data_json FROM form_drafts
             WHERE tin = ?1 AND form_code = ?2
               AND taxable_year = ?3 AND period_key = ?4
             ORDER BY updated_at DESC, id DESC
             LIMIT 1",
        )?;
        let mut rows = stmt.query(params![tin, form_code, year as i64, period_key])?;

        if let Some(row) = rows.next()? {
            let json: String = row.get(0)?;
            let draft: T = serde_json::from_str(&json)?;
            Ok(Some(draft))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forms::form_2551q::{Item13Election, Schedule1Row};
    use crate::profile::{IncomeTaxElection, TaxElectionHistory, TaxpayerProfile};
    use rusqlite::Connection;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
    struct TestDraft {
        value: i32,
    }

    /// `submissions.list` must keep a return after BIR confirms it.
    #[test]
    fn filed_listing_keeps_confirmed_rows_that_the_queue_listing_drops() {
        let db = test_db();
        let profile = test_profile();
        let draft = editable_1601c_draft(&profile);
        db.save_1601c_draft(&draft).expect("draft");
        let mut queued = draft.clone();
        queued.transition_to_queued().expect("queue");
        db.save_queued_1601c_draft(&queued).expect("queued");
        assert_eq!(db.list_all_queued_submissions().unwrap().len(), 1);
        assert_eq!(db.list_all_filed_submissions().unwrap().len(), 1);

        db.conn
            .execute(
                "UPDATE form_drafts SET status = 'Confirmed' WHERE tin = ?1",
                rusqlite::params![queued.tin],
            )
            .expect("settle the row");
        assert!(db.list_all_queued_submissions().unwrap().is_empty());
        let filed = db.list_all_filed_submissions().unwrap();
        assert_eq!(filed.len(), 1);
        assert_eq!(filed[0].status, FilingStatus::Confirmed);
    }

    /// A form's returns list newest first with their keys; the unsubmitted
    /// ones are the drafts still in Draft.
    #[test]
    fn unsubmitted_drafts_leave_out_queued_returns_and_other_forms() {
        let db = test_db();
        let profile = test_profile();
        let january = Form1601CDraft::new_from_profile(&profile, 2026, 1);
        db.save_1601c_draft(&january).expect("january");
        let february = Form1601CDraft::new_from_profile(&profile, 2026, 2);
        db.save_1601c_draft(&february).expect("february");
        db.conn
            .execute(
                "UPDATE form_drafts SET status = 'Queued' WHERE quarter = 2",
                [],
            )
            .expect("queue february");
        db.conn
            .execute(
                "UPDATE form_drafts SET updated_at = '2000-01-01' WHERE quarter = 1",
                [],
            )
            .expect("age january");
        let tin = january.tin.clone();
        let all = db.list_form_returns(&tin, "1601C").unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].0.month, Some(2), "newest first");
        assert_eq!(all[0].1, Some(2));
        let open = db.unsubmitted_drafts(&tin, "1601C").unwrap();
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].0.month, Some(1));
        assert_eq!(open[0].0.status, FilingStatus::Draft);
        assert!(db.unsubmitted_drafts(&tin, "2551Q").unwrap().is_empty());
    }

    /// The Calendar reads a filed return's own total amount payable to decide
    /// between "Mark as paid" and its done state.
    #[test]
    fn draft_amount_payable_reads_the_saved_1601c_item_36() {
        let db = test_db();
        let profile = test_profile();
        let zero_due = editable_1601c_draft(&profile);
        let zero_id = db.save_1601c_draft(&zero_due).expect("zero-due draft");
        assert_eq!(db.draft_amount_payable(zero_id).unwrap(), Some(0.0));

        let mut owing = Form1601CDraft::new_from_profile(&profile, 2026, 6);
        owing.set_any_taxes_withheld(true);
        owing.tax_14_total_compensation = 15_000.0;
        owing.tax_25_total_taxes_withheld = 1_500.0;
        // Pin Items 32-34 at zero: the period is past its deadline, and this
        // test is not about penalties, so they must not grow with the clock.
        for component in crate::penalties::PenaltyComponent::ALL {
            owing.set_penalty_override(component, Some(0.0));
        }
        owing.compute();
        let owing_id = db.save_1601c_draft(&owing).expect("owing draft");
        assert_eq!(db.draft_amount_payable(owing_id).unwrap(), Some(1_500.0));

        assert_eq!(db.draft_amount_payable(owing_id + 1_000).unwrap(), None);
    }

    /// A generic save replaces a stored row only while it is a Draft: a
    /// return that left Draft (queued, submitted, confirmed, paid) is the
    /// filed record and is never saved over. Draft saves keep working.
    #[test]
    fn a_generic_save_never_replaces_a_return_that_left_draft() {
        let db = test_db();
        let period = FilingPeriod::Quarterly(2);
        let save = |value: i32| {
            db.save_form_draft_v2(
                "123456789000",
                "2550Q",
                2026,
                &period,
                &FilingStatus::Draft,
                &TestDraft { value },
            )
        };
        let id = save(1).expect("a new draft");
        assert_eq!(save(2).expect("a draft saved again"), id);
        let stored = |db: &Database| -> (String, String) {
            db.conn
                .query_row(
                    "SELECT status, data_json FROM form_drafts WHERE id = ?1",
                    params![id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap()
        };
        assert_eq!(stored(&db).1, r#"{"value":2}"#);
        for status in ["Queued", "Submitted", "Confirmed", "Paid"] {
            db.conn
                .execute(
                    "UPDATE form_drafts SET status = ?1, data_json = '{\"value\":2}' WHERE id = ?2",
                    params![status, id],
                )
                .unwrap();
            let error = save(3).expect_err(status).to_string();
            assert!(error.contains("no longer a draft"), "{status}: {error}");
            assert_eq!(
                stored(&db),
                (status.to_string(), r#"{"value":2}"#.to_string())
            );
            // The legacy entry point reaches the same guard.
            assert!(
                db.save_form_draft(
                    "123456789000",
                    "2550Q",
                    2026,
                    Some(2),
                    &FilingStatus::Draft,
                    &TestDraft { value: 4 }
                )
                .is_err(),
                "{status}"
            );
            assert_eq!(stored(&db).1, r#"{"value":2}"#);
        }
        // Back in Draft (the user's revert), it saves again.
        db.conn
            .execute(
                "UPDATE form_drafts SET status = 'Draft' WHERE id = ?1",
                params![id],
            )
            .unwrap();
        assert_eq!(save(5).expect("a draft again"), id);
        assert_eq!(stored(&db).1, r#"{"value":5}"#);
    }

    fn test_db() -> Database {
        let conn = Connection::open_in_memory().unwrap();
        super::super::migrations::migrate_database(&conn).unwrap();
        Database {
            conn,
            _owner_lock: None,
        }
    }

    fn test_profile() -> TaxpayerProfile {
        serde_json::from_value(serde_json::json!({
            "id": null,
            "full_name": "Test Taxpayer",
            "tin": {
                "segment1": "123",
                "segment2": "456",
                "segment3": "789",
                "branch": "000"
            },
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

    fn insert_test_profile(db: &Database, profile: &TaxpayerProfile) {
        let mut persisted = profile.clone();
        persisted.ensure_profile_version_ledger();
        insert_raw_test_profile(db, &persisted);
    }

    fn insert_raw_test_profile(db: &Database, profile: &TaxpayerProfile) {
        db.conn
            .execute(
                "INSERT INTO profiles (tin, data_json) VALUES (?1, ?2)",
                params![profile.tin.full(), serde_json::to_string(profile).unwrap()],
            )
            .unwrap();
    }

    fn queued_eight_percent_draft(profile: &TaxpayerProfile) -> Form2551QDraft {
        let mut effective_profile = profile.clone();
        effective_profile.ensure_profile_version_ledger();
        let mut draft = Form2551QDraft::new_from_effective_profile(&effective_profile, 2026, 1);
        draft.item_13_election = Item13Election::EightPercent;
        draft
            .transition_to_queued()
            .expect("a NIL PT010 Q1 8% election should queue");
        draft
    }

    fn queued_graduated_draft(profile: &TaxpayerProfile) -> Form2551QDraft {
        let mut effective_profile = profile.clone();
        effective_profile.ensure_profile_version_ledger();
        let mut draft = Form2551QDraft::new_from_effective_profile(&effective_profile, 2026, 1);
        draft.item_13_election = Item13Election::Graduated;
        draft
            .transition_to_queued()
            .expect("a Q1 graduated election should queue");
        // Pin the queue time a few minutes before the fixed `submitted_at`
        // below (12:00:00 PHT) so receipt windows are deterministic.
        draft
            .queue_authorization
            .as_mut()
            .expect("queueing records an authorization")
            .authorized_at = "2026-04-25T03:55:00+00:00".to_string();
        draft
    }

    fn submitted_2551q_draft(db: &Database, profile: &TaxpayerProfile) -> Form2551QDraft {
        insert_test_profile(db, profile);
        let queued = queued_graduated_draft(profile);
        db.save_queued_2551q_draft_and_election(&queued)
            .expect("queued draft should persist before the test submission");
        let (mut claimed, token) = match db
            .claim_queued_2551q_submission(
                &queued.tin,
                queued.taxable_year,
                queued.quarter,
                &queued.queued_submission_fingerprint,
                &queued.next_retry_at,
                queued.submission_attempts,
            )
            .expect("queued draft claim should execute")
        {
            Claim2551QSubmissionResult::Claimed { draft, token } => (draft, token),
            _ => panic!("the exact queued generation should be claimed"),
        };
        let filename = claimed.default_submission_filename();
        claimed.transition_to_submitted(filename);
        claimed.submitted_at = Some("2026-04-25T04:00:00+00:00".to_string());
        db.finish_claimed_2551q_submission(&claimed, &token)
            .expect("the claimed draft should become Submitted");
        db.get_2551q_draft(&claimed.tin, claimed.taxable_year, claimed.quarter)
            .unwrap()
            .unwrap()
    }

    fn insert_2551q_receipt(
        db: &Database,
        submitted: &Form2551QDraft,
        filename: &str,
        received_date: &str,
        received_time: &str,
    ) -> i64 {
        db.conn
            .execute(
                "INSERT INTO submission_receipts
                    (filename, tin, form_type, period, received_date, received_time, raw_text)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'test receipt')",
                params![
                    filename,
                    &submitted.tin,
                    AUDITED_2551Q_RECEIPT_FORM,
                    submitted.period_code(),
                    received_date,
                    received_time
                ],
            )
            .unwrap();
        db.conn.last_insert_rowid()
    }

    fn assert_2551q_confirmation_rejected_without_draft_mutation(
        db: &Database,
        confirmation: &Form2551QDraft,
    ) {
        let before = db
            .conn
            .query_row(
                "SELECT status, data_json, updated_at FROM form_drafts
                 WHERE tin = ?1 AND form_code = '2551Q'
                   AND taxable_year = ?2 AND quarter = ?3",
                params![
                    &confirmation.tin,
                    i64::from(confirmation.taxable_year),
                    i64::from(confirmation.quarter)
                ],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .unwrap();
        assert!(db.save_confirmed_2551q_draft(confirmation).is_err());
        let after = db
            .conn
            .query_row(
                "SELECT status, data_json, updated_at FROM form_drafts
                 WHERE tin = ?1 AND form_code = '2551Q'
                   AND taxable_year = ?2 AND quarter = ?3",
                params![
                    &confirmation.tin,
                    i64::from(confirmation.taxable_year),
                    i64::from(confirmation.quarter)
                ],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(after, before);
    }

    fn editable_1601c_draft(profile: &TaxpayerProfile) -> Form1601CDraft {
        let mut draft = Form1601CDraft::new_from_profile(profile, 2026, 5);
        draft.any_taxes_withheld = false;
        draft.compute();
        draft
    }

    fn queued_1601c_draft(profile: &TaxpayerProfile) -> Form1601CDraft {
        let mut draft = editable_1601c_draft(profile);
        draft
            .transition_to_queued()
            .expect("the reviewed 1601C draft should queue");
        draft
    }

    #[test]
    fn save_and_reopen_2551q_preserves_ten_distinct_printable_schedule_rows() {
        let db = test_db();
        let profile = test_profile();
        let mut draft = Form2551QDraft::new_from_profile(&profile, 2026, 1);
        draft.schedule_1 = [
            "PT010", "PT040", "PT041", "PT060", "PT070", "PT090", "PT140", "PT150", "PT160",
            "PT170",
        ]
        .into_iter()
        .enumerate()
        .map(|(index, code)| {
            let mut row = Schedule1Row::new(code).expect("test ATC must be canonical");
            row.taxable_amount = (index as f64 + 1.0) * 1_000.0;
            row
        })
        .collect();
        assert!(draft.ensure_required_schedule_attachment_sheets());
        draft.recompute(None);

        db.save_2551q_draft(&draft)
            .expect("ten-row printable draft should save without claiming XML fileability");
        let reopened = db
            .get_2551q_draft(&draft.tin, draft.taxable_year, draft.quarter)
            .expect("saved draft lookup should succeed")
            .expect("saved draft should exist");

        assert_eq!(
            reopened
                .schedule_1
                .iter()
                .map(|row| row.atc.as_str())
                .collect::<Vec<_>>(),
            vec![
                "PT010", "PT040", "PT041", "PT060", "PT070", "PT090", "PT140", "PT150", "PT160",
                "PT170"
            ]
        );
        assert_eq!(
            reopened
                .schedule_1
                .iter()
                .map(|row| row.taxable_amount)
                .collect::<Vec<_>>(),
            vec![
                1_000.0, 2_000.0, 3_000.0, 4_000.0, 5_000.0, 6_000.0, 7_000.0, 8_000.0, 9_000.0,
                10_000.0
            ]
        );
        assert_eq!(
            reopened
                .schedule_1
                .iter()
                .map(|row| row.tax_due)
                .collect::<Vec<_>>(),
            vec![
                30.0, 60.0, 90.0, 80.0, 150.0, 600.0, 1_260.0, 1_440.0, 900.0, 1_500.0
            ]
        );
        assert_eq!(reopened.number_of_attached_sheets, 1);
        assert_eq!(reopened.required_schedule_attachment_sheets(), 1);
        assert_eq!(reopened.total_tax_due, 6_110.0);
    }

    #[test]
    fn save_and_reopen_1601c_preserves_all_three_schedule_rows() {
        use crate::forms::form_1601c::{Form1601CDraft, Form1601CSchedule1Row};

        let db = test_db();
        let profile = test_profile();
        let mut draft = Form1601CDraft::new_from_profile(&profile, 2026, 4);
        draft.any_taxes_withheld = false;
        draft.auto_compute_penalties = false;
        draft.tax_relief = true;
        draft.tax_relief_specification = "International Tax Treaty".to_string();
        draft.schedule_1 = (1..=3)
            .map(|index| Form1601CSchedule1Row {
                previous_month: format!("{index:02}/2026"),
                date_paid: format!("{index:02}/10/2026"),
                drawee_bank_code_or_agency: format!("AAB-{index}"),
                payment_number: format!("REF-{index}"),
                tax_paid: f64::from(index) * 100.0,
                should_be_tax_due: f64::from(index) * 125.0,
                adjustment: 0.0,
            })
            .collect();
        draft.compute();

        db.save_1601c_draft(&draft)
            .expect("three-row 1601-C draft should save");
        let reopened = db
            .get_1601c_draft(&draft.tin, draft.taxable_year, draft.month)
            .expect("1601-C lookup should succeed")
            .expect("saved 1601-C draft should exist");

        assert_eq!(reopened.schedule_1, draft.schedule_1);
        assert_eq!(
            reopened.tax_relief_specification,
            "International Tax Treaty"
        );
        assert_eq!(reopened.tax_26_adjustment, 150.0);
    }

    #[test]
    fn save_and_reopen_1601c_preserves_government_category() {
        use crate::forms::FormValidator;
        use crate::forms::form_1601c::Form1601CDraft;

        let db = test_db();
        let profile = test_profile();
        let mut draft = Form1601CDraft::new_from_profile(&profile, 2026, 4);
        draft.any_taxes_withheld = false;
        assert_eq!(draft.category_of_agent, "P");
        draft.category_of_agent = "G".to_string();
        draft.compute();

        db.save_1601c_draft(&draft)
            .expect("government category 1601-C draft should save");
        let reopened = db
            .get_1601c_draft(&draft.tin, draft.taxable_year, draft.month)
            .expect("1601-C lookup should succeed")
            .expect("saved 1601-C draft should exist");
        assert_eq!(reopened.category_of_agent, "G");
        assert!(
            reopened
                .validate()
                .iter()
                .all(|(field, _)| field != "category_of_agent")
        );
    }

    #[test]
    fn editable_1601c_to_exact_xml_queue_claim_and_completion_is_immutable() {
        let db = test_db();
        let profile = test_profile();
        let editable = editable_1601c_draft(&profile);
        db.save_1601c_draft(&editable)
            .expect("editable 1601C should save");

        let mut queued = editable.clone();
        queued
            .transition_to_queued()
            .expect("validated editable 1601C should queue");
        let queued_fields = queued.to_bir_field_map();
        let xml = queued
            .try_to_bir_xml_payload()
            .expect("queued 1601C should produce checked XML");
        assert_eq!(
            crate::bir_xml::parse_bir_xml_with_codec_checked(
                &xml,
                bir_rules::serialization::BodyCodec::Utf8PercentRfc3986Unreserved,
            )
            .unwrap(),
            queued_fields
        );
        assert_eq!(
            queued.default_submission_filename(),
            "123456789000-1601Cv2018-052026#test@example.com#.xml"
        );

        db.save_queued_1601c_draft(&queued)
            .expect("reviewed queue snapshot should persist");
        let stored = db
            .get_1601c_draft(&queued.tin, queued.taxable_year, queued.month)
            .unwrap()
            .unwrap();
        assert_eq!(stored.status, FilingStatus::Queued);
        assert_eq!(stored.to_bir_field_map(), queued_fields);
        assert_eq!(
            stored.queued_submission_fingerprint,
            queued.queued_submission_fingerprint
        );
        let mut tampered_cancellation = stored.clone();
        tampered_cancellation.tax_14_total_compensation = 42.0;
        assert!(
            db.cancel_queued_1601c_submission(&tampered_cancellation)
                .is_err()
        );

        let mut stale_editable = editable.clone();
        stale_editable.tax_14_total_compensation = 99_999.0;
        assert!(db.save_1601c_draft(&stale_editable).is_err());
        assert!(
            db.save_form_draft_v2(
                &stale_editable.tin,
                "1601C",
                stale_editable.taxable_year,
                &FilingPeriod::Monthly(stale_editable.month),
                &FilingStatus::Draft,
                &stale_editable,
            )
            .is_err()
        );
        assert!(
            db.save_form_draft(
                &stale_editable.tin,
                "1601C",
                stale_editable.taxable_year,
                Some(stale_editable.month),
                &FilingStatus::Draft,
                &stale_editable,
            )
            .is_err()
        );
        assert!(
            db.save_imported_form(
                &stale_editable.tin,
                "1601C",
                stale_editable.taxable_year,
                None,
                Some(stale_editable.month),
            )
            .is_err()
        );
        assert_eq!(
            db.get_1601c_draft(&queued.tin, queued.taxable_year, queued.month)
                .unwrap()
                .unwrap()
                .to_bir_field_map(),
            queued_fields
        );

        let expected_fingerprint = stored.queued_submission_fingerprint.clone();
        let expected_retry = stored.next_retry_at.clone();
        let expected_attempts = stored.submission_attempts;
        let (mut claimed, token) = match db
            .claim_queued_1601c_submission(
                &stored.tin,
                stored.taxable_year,
                stored.month,
                &expected_fingerprint,
                &expected_retry,
                expected_attempts,
            )
            .unwrap()
        {
            Claim1601CSubmissionResult::Claimed { draft, token } => (draft, token),
            _ => panic!("the unchanged queue generation should be claimed"),
        };
        assert_eq!(claimed.to_bir_field_map(), queued_fields);
        assert_eq!(
            claimed.submission_claim_token.as_deref(),
            Some(token.as_str())
        );

        let mut stale_cancellation = queued.clone();
        stale_cancellation.revert_to_draft();
        assert!(
            !db.replace_unclaimed_queued_1601c_submission(
                &stale_cancellation,
                &expected_fingerprint,
                &expected_retry,
                expected_attempts,
            )
            .unwrap(),
            "a claimed queue generation must reject stale cancellation"
        );
        assert!(db.save_1601c_draft(&stale_editable).is_err());

        let filename = claimed.default_submission_filename();
        let mut wrong_filename = claimed.clone();
        wrong_filename.transition_to_submitted("wrong.xml".to_string());
        assert!(
            db.finish_claimed_1601c_submission(&wrong_filename, &token)
                .is_err()
        );
        claimed.transition_to_submitted(filename.clone());
        assert!(
            db.finish_claimed_1601c_submission(&claimed, "wrong-token")
                .is_err()
        );
        db.finish_claimed_1601c_submission(&claimed, &token)
            .expect("the claim owner should finalize the exact submitted snapshot");

        let submitted = db
            .get_1601c_draft(&queued.tin, queued.taxable_year, queued.month)
            .unwrap()
            .unwrap();
        assert_eq!(submitted.status, FilingStatus::Sent);
        assert_eq!(submitted.to_bir_field_map(), queued_fields);
        assert_eq!(
            submitted.submission_filename.as_deref(),
            Some(filename.as_str())
        );
        assert!(submitted.submission_claim_token.is_none());
        assert!(db.save_1601c_draft(&stale_editable).is_err());
    }

    #[test]
    fn claimed_1601c_revalidation_atomically_rejects_tampered_queue_data() {
        let db = test_db();
        let profile = test_profile();
        let queued = queued_1601c_draft(&profile);
        db.save_queued_1601c_draft(&queued).unwrap();

        let expected_fingerprint = queued.queued_submission_fingerprint.clone();
        let expected_retry = queued.next_retry_at.clone();
        let expected_attempts = queued.submission_attempts;
        let mut tampered = db
            .get_1601c_draft(&queued.tin, queued.taxable_year, queued.month)
            .unwrap()
            .unwrap();
        tampered.tax_14_total_compensation = 42.0;
        db.conn
            .execute(
                "UPDATE form_drafts SET data_json = ?1
                 WHERE tin = ?2 AND form_code = '1601C'
                   AND taxable_year = ?3 AND quarter = ?4",
                params![
                    serde_json::to_string(&tampered).unwrap(),
                    &queued.tin,
                    i64::from(queued.taxable_year),
                    i64::from(queued.month)
                ],
            )
            .unwrap();

        match db
            .claim_queued_1601c_submission(
                &queued.tin,
                queued.taxable_year,
                queued.month,
                &expected_fingerprint,
                &expected_retry,
                expected_attempts,
            )
            .unwrap()
        {
            Claim1601CSubmissionResult::Rejected { draft, errors } => {
                assert_eq!(draft.status, FilingStatus::Draft);
                assert!(errors.iter().any(|(field, _)| {
                    field == "queued_submission_fingerprint" || field == "tax_22_total_taxable"
                }));
            }
            _ => panic!("tampered queue data must be rejected before a claim"),
        }

        let rejected = db
            .get_1601c_draft(&queued.tin, queued.taxable_year, queued.month)
            .unwrap()
            .unwrap();
        assert_eq!(rejected.status, FilingStatus::Draft);
        assert!(rejected.submission_claim_token.is_none());
        assert!(
            rejected
                .submission_error
                .as_deref()
                .is_some_and(|message| message.contains("queue revalidation"))
        );
    }

    #[test]
    fn exact_unclaimed_1601c_queue_can_be_canceled_edited_and_requeued() {
        let db = test_db();
        let profile = test_profile();
        let queued = queued_1601c_draft(&profile);
        db.save_queued_1601c_draft(&queued).unwrap();

        let mut canceled = db.cancel_queued_1601c_submission(&queued).unwrap();
        assert_eq!(canceled.status, FilingStatus::Draft);
        assert!(canceled.queued_submission_fingerprint.is_none());
        canceled.tax_14_total_compensation = 123.45;
        canceled.compute();
        db.save_1601c_draft(&canceled).unwrap();

        canceled.transition_to_queued().unwrap();
        db.save_queued_1601c_draft(&canceled).unwrap();
        let requeued = db
            .get_1601c_draft(&canceled.tin, canceled.taxable_year, canceled.month)
            .unwrap()
            .unwrap();
        assert_eq!(requeued.status, FilingStatus::Queued);
        assert_eq!(requeued.to_bir_field_map(), canceled.to_bir_field_map());
    }

    #[test]
    fn claimed_1601c_cannot_cancel_but_human_confirmed_release_returns_draft() {
        let db = test_db();
        let queued = queued_1601c_draft(&test_profile());
        db.save_queued_1601c_draft(&queued).unwrap();
        let stored = db
            .get_1601c_draft(&queued.tin, queued.taxable_year, queued.month)
            .unwrap()
            .unwrap();
        let (claimed, token) = match db
            .claim_queued_1601c_submission(
                &stored.tin,
                stored.taxable_year,
                stored.month,
                &stored.queued_submission_fingerprint,
                &stored.next_retry_at,
                stored.submission_attempts,
            )
            .unwrap()
        {
            Claim1601CSubmissionResult::Claimed { draft, token } => (draft, token),
            _ => panic!("queued 1601C should be claimed"),
        };
        assert!(
            db.cancel_queued_1601c_submission(&claimed).is_err(),
            "claimed queued 1601C must still refuse form.revert_draft cancel"
        );
        let wrong_reason = db.release_abandoned_claimed_1601c_submission(
            &claimed.tin,
            claimed.taxable_year,
            claimed.month,
            "because I want to",
        );
        assert!(wrong_reason.is_err());

        let released = db
            .release_abandoned_claimed_1601c_submission(
                &claimed.tin,
                claimed.taxable_year,
                claimed.month,
                ABANDONED_CLAIM_RELEASE_REASON,
            )
            .unwrap();
        let AbandonedClaimRelease::Released {
            previous_status,
            previous_claim_present,
            draft,
            reason,
            ..
        } = released
        else {
            panic!("claimed 1601C should release to Draft");
        };
        assert_eq!(previous_status, FilingStatus::Queued);
        assert!(previous_claim_present);
        assert_eq!(reason, ABANDONED_CLAIM_RELEASE_REASON);
        assert_eq!(draft.status, FilingStatus::Draft);
        assert!(draft.submission_claim_token.is_none());
        assert!(draft.submission_claimed_at.is_none());
        assert!(
            draft
                .submission_error
                .as_deref()
                .is_some_and(|message| message.contains("Abandoned claim released"))
        );
        assert!(
            !draft
                .submission_error
                .as_deref()
                .is_some_and(|message| message.contains("Automatic retry is disabled"))
        );

        let persisted = db
            .get_1601c_draft(&draft.tin, draft.taxable_year, draft.month)
            .unwrap()
            .unwrap();
        assert_eq!(persisted.status, FilingStatus::Draft);
        assert!(persisted.submission_claim_token.is_none());

        let mut finished = claimed.clone();
        finished.transition_to_submitted(claimed.default_submission_filename());
        assert!(
            db.finish_claimed_1601c_submission(&finished, &token)
                .is_err(),
            "finish_claimed must not invent Submitted after a human release"
        );

        let again = db
            .release_abandoned_claimed_1601c_submission(
                &draft.tin,
                draft.taxable_year,
                draft.month,
                ABANDONED_CLAIM_RELEASE_REASON,
            )
            .unwrap();
        assert!(matches!(again, AbandonedClaimRelease::AlreadyClear { .. }));

        let mut other = editable_1601c_draft(&test_profile());
        other.month = 6;
        other.compute();
        other.transition_to_queued().unwrap();
        db.save_queued_1601c_draft(&other).unwrap();
        let refuse_unclaimed = db.release_abandoned_claimed_1601c_submission(
            &other.tin,
            other.taxable_year,
            other.month,
            ABANDONED_CLAIM_RELEASE_REASON,
        );
        assert!(
            refuse_unclaimed.is_err(),
            "unclaimed queued 1601C must keep using cancel, not abandoned release"
        );
    }

    #[test]
    fn finish_claimed_1601c_path_is_unchanged_without_release() {
        let db = test_db();
        let queued = queued_1601c_draft(&test_profile());
        db.save_queued_1601c_draft(&queued).unwrap();
        let stored = db
            .get_1601c_draft(&queued.tin, queued.taxable_year, queued.month)
            .unwrap()
            .unwrap();
        let (mut claimed, token) = match db
            .claim_queued_1601c_submission(
                &stored.tin,
                stored.taxable_year,
                stored.month,
                &stored.queued_submission_fingerprint,
                &stored.next_retry_at,
                stored.submission_attempts,
            )
            .unwrap()
        {
            Claim1601CSubmissionResult::Claimed { draft, token } => (draft, token),
            _ => panic!("queued 1601C should be claimed"),
        };
        let filename = claimed.default_submission_filename();
        claimed.transition_to_submitted(filename.clone());
        db.finish_claimed_1601c_submission(&claimed, &token)
            .expect("finish_claimed remains the success path");
        let submitted = db
            .get_1601c_draft(&queued.tin, queued.taxable_year, queued.month)
            .unwrap()
            .unwrap();
        assert_eq!(submitted.status, FilingStatus::Sent);
        assert!(submitted.submission_claim_token.is_none());
        assert!(
            db.release_abandoned_claimed_1601c_submission(
                &submitted.tin,
                submitted.taxable_year,
                submitted.month,
                ABANDONED_CLAIM_RELEASE_REASON,
            )
            .is_err(),
            "must not wipe a Submitted 1601C"
        );
    }

    #[test]
    fn orphaned_1601c_claimed_at_blocks_replacement_cancellation_and_reclaim() {
        let db = test_db();
        let queued = queued_1601c_draft(&test_profile());
        db.save_queued_1601c_draft(&queued).unwrap();
        let stored = db
            .get_1601c_draft(&queued.tin, queued.taxable_year, queued.month)
            .unwrap()
            .unwrap();
        let expected_fingerprint = stored.queued_submission_fingerprint.clone();
        let expected_retry = stored.next_retry_at.clone();
        let expected_attempts = stored.submission_attempts;

        let mut claimed_replacement = stored.clone();
        claimed_replacement.submission_claimed_at = Some("2026-05-10T04:00:00+00:00".to_string());
        assert!(
            db.replace_unclaimed_queued_1601c_submission(
                &claimed_replacement,
                &expected_fingerprint,
                &expected_retry,
                expected_attempts,
            )
            .is_err(),
            "replacement input carrying only claimed_at is still claimed"
        );

        let mut orphaned = stored;
        orphaned.submission_claimed_at = Some("2026-05-10T04:00:00+00:00".to_string());
        let orphaned_json = serde_json::to_string(&orphaned).unwrap();
        db.conn
            .execute(
                "UPDATE form_drafts SET data_json = ?1
                 WHERE tin = ?2 AND form_code = '1601C'
                   AND taxable_year = ?3 AND quarter = ?4",
                params![
                    &orphaned_json,
                    &orphaned.tin,
                    i64::from(orphaned.taxable_year),
                    i64::from(orphaned.month)
                ],
            )
            .unwrap();

        assert!(
            db.cancel_queued_1601c_submission(&orphaned).is_err(),
            "orphaned claimed_at must make cancellation fail closed"
        );
        let mut retry = orphaned.clone();
        retry.submission_claimed_at = None;
        retry.record_submission_failure("must not retry orphaned claim".to_string());
        assert!(
            !db.replace_unclaimed_queued_1601c_submission(
                &retry,
                &expected_fingerprint,
                &expected_retry,
                expected_attempts,
            )
            .unwrap()
        );
        assert!(matches!(
            db.claim_queued_1601c_submission(
                &orphaned.tin,
                orphaned.taxable_year,
                orphaned.month,
                &expected_fingerprint,
                &expected_retry,
                expected_attempts,
            )
            .unwrap(),
            Claim1601CSubmissionResult::Superseded
        ));
        let after = db
            .get_1601c_draft(&orphaned.tin, orphaned.taxable_year, orphaned.month)
            .unwrap()
            .unwrap();
        assert_eq!(serde_json::to_string(&after).unwrap(), orphaned_json);
    }

    #[test]
    fn token_only_1601c_claim_cannot_finalize_submission() {
        let db = test_db();
        let queued = queued_1601c_draft(&test_profile());
        db.save_queued_1601c_draft(&queued).unwrap();
        let mut orphaned = db
            .get_1601c_draft(&queued.tin, queued.taxable_year, queued.month)
            .unwrap()
            .unwrap();
        let claim_token = "token-without-claimed-at";
        orphaned.submission_claim_token = Some(claim_token.to_string());
        orphaned.submission_claimed_at = None;
        let orphaned_json = serde_json::to_string(&orphaned).unwrap();
        db.conn
            .execute(
                "UPDATE form_drafts SET data_json = ?1
                 WHERE tin = ?2 AND form_code = '1601C'
                   AND taxable_year = ?3 AND quarter = ?4",
                params![
                    &orphaned_json,
                    &orphaned.tin,
                    i64::from(orphaned.taxable_year),
                    i64::from(orphaned.month)
                ],
            )
            .unwrap();

        let mut submitted = orphaned.clone();
        submitted.transition_to_submitted(submitted.default_submission_filename());
        assert!(
            db.finish_claimed_1601c_submission(&submitted, claim_token)
                .is_err()
        );
        let after = db
            .get_1601c_draft(&orphaned.tin, orphaned.taxable_year, orphaned.month)
            .unwrap()
            .unwrap();
        assert_eq!(serde_json::to_string(&after).unwrap(), orphaned_json);
    }

    #[test]
    fn save_and_reopen_0605_preserves_independent_dates_codes_and_pdf_only_details() {
        use crate::forms::FormValidator;
        use crate::forms::form_0605::{
            Form0605Date, Form0605Draft, Form0605FilingBasis, Form0605MannerOfPayment,
            Form0605ReviewedAtc, Form0605ReviewedTaxType, Form0605TypeOfPayment,
        };

        let db = test_db();
        let profile = test_profile();
        let mut draft = Form0605Draft::new_from_profile(&profile, 2025, 1);
        // The official TIN check digit must hold for a valid return.
        draft.tin = "123-456-788-00000".to_string();
        // A December year end is a calendar year on the official page.
        draft.filing_basis = Form0605FilingBasis::Calendar;
        draft.quarter = 1;
        draft.year_end_month = 12;
        draft.due_date = Some(Form0605Date::new(2025, 12, 31).unwrap());
        draft.return_period = Some(Form0605Date::new(2025, 12, 31).unwrap());
        draft.number_of_sheets = 10;
        draft.select_reviewed_atc(Form0605ReviewedAtc::Ii011);
        draft.select_reviewed_tax_type(Form0605ReviewedTaxType::It);
        draft.manner_of_payment = Some(Form0605MannerOfPayment::Others);
        draft.other_manner_description = "MANUAL PAYMENT".to_string();
        draft.type_of_payment = Some(Form0605TypeOfPayment::Installment);
        draft.number_of_installments = Some(10);
        draft.item_19_basic_tax_or_payment = 1_000.0;
        draft.item_20a_surcharge = 10.0;
        draft.item_20b_interest = 20.0;
        draft.item_20c_compromise = 1_000.0;
        crate::forms::queueable::QueueableForm::keep_entered_penalties(&mut draft);
        draft.signatures.taxpayer_or_authorized_representative = "TEST TAXPAYER".to_string();
        draft.signatures.title_or_position = "OWNER".to_string();
        draft.payment_details.check.drawee_bank_or_agency = "AAB".to_string();
        draft.payment_details.check.number = "CHECK-24".to_string();
        draft.payment_details.check.date = "12/31/2025".to_string();
        draft.payment_details.check.amount = Some(2_030.0);
        draft.recompute();
        assert!(
            draft.validate().is_empty(),
            "test draft must be valid: {:?}",
            draft.validate()
        );

        db.save_form_draft(
            &draft.tin,
            "0605",
            draft.taxable_year,
            Some(draft.month),
            &draft.lifecycle.status,
            &draft,
        )
        .expect("0605 draft should save");
        let reopened = db
            .get_form_draft::<Form0605Draft>(
                &draft.tin,
                "0605",
                draft.taxable_year,
                Some(draft.month),
            )
            .expect("0605 lookup should succeed")
            .expect("saved 0605 draft should exist");

        assert_eq!(reopened, draft);
        assert_eq!(reopened.item_20d_total_penalties, 1_030.0);
        assert_eq!(reopened.item_21_total_amount_payable, 2_030.0);
    }

    #[test]
    fn save_and_reopen_0619e_preserves_manual_due_day_and_fixed_payment_rows() {
        use crate::forms::form_0619e::{
            Form0619EDraft, Form0619EPaymentRow, WithholdingAgentCategory,
        };

        let db = test_db();
        let profile = test_profile();
        let mut draft = Form0619EDraft::new_from_profile(&profile, 2026, 12);
        draft.due_day = Some(10);
        draft.withholding_agent_category = WithholdingAgentCategory::Government;
        draft.any_taxes_withheld = true;
        draft.item_14_amount_of_remittance = 1_000.0;
        draft.item_17a_surcharge = 100.0;
        draft.item_17b_interest = 30.0;
        draft.item_17c_compromise = 100.0;
        crate::forms::queueable::QueueableForm::keep_entered_penalties(&mut draft);
        draft.payment_details.others = Form0619EPaymentRow {
            drawee_bank_or_agency: "AAB".to_string(),
            number: "REF-22".to_string(),
            date: "01/10/2027".to_string(),
            amount: Some(1_230.0),
        };
        draft.payment_details.others_description = "OTHER PAYMENT".to_string();
        draft.recompute();

        db.save_form_draft(
            &draft.tin,
            "0619E",
            draft.taxable_year,
            Some(draft.month),
            &draft.lifecycle.status,
            &draft,
        )
        .expect("0619-E draft should save");
        let reopened = db
            .get_form_draft::<Form0619EDraft>(
                &draft.tin,
                "0619E",
                draft.taxable_year,
                Some(draft.month),
            )
            .expect("0619-E lookup should succeed")
            .expect("saved 0619-E draft should exist");

        assert_eq!(reopened.due_day, Some(10));
        assert_eq!(reopened.due_month_and_year(), (1, 2027));
        assert_eq!(reopened.payment_details, draft.payment_details);
        assert_eq!(reopened.item_18_total_amount_of_remittance, 1_230.0);
    }

    #[test]
    fn save_and_reopen_0619f_preserves_manual_due_day_and_fixed_payment_rows() {
        use crate::forms::form_0619f::{
            Form0619FDraft, Form0619FPaymentRow, WithholdingAgentCategory,
        };

        let db = test_db();
        let profile = test_profile();
        let mut draft = Form0619FDraft::new_from_profile(&profile, 2026, 12);
        draft.due_day = Some(10);
        draft.withholding_agent_category = WithholdingAgentCategory::Government;
        draft.any_taxes_withheld = true;
        draft.item_13_interest_final_tax_withheld = 1_000.0;
        draft.item_18a_surcharge = 100.0;
        draft.item_18b_interest = 30.0;
        draft.item_18c_compromise = 100.0;
        crate::forms::queueable::QueueableForm::keep_entered_penalties(&mut draft);
        draft.payment_details.others = Form0619FPaymentRow {
            drawee_bank_or_agency: "AAB".to_string(),
            number: "REF-23".to_string(),
            date: "01/10/2027".to_string(),
            amount: Some(1_230.0),
        };
        draft.payment_details.others_description = "OTHER PAYMENT".to_string();
        draft.recompute();

        db.save_form_draft(
            &draft.tin,
            "0619F",
            draft.taxable_year,
            Some(draft.month),
            &draft.lifecycle.status,
            &draft,
        )
        .expect("0619-F draft should save");
        let reopened = db
            .get_form_draft::<Form0619FDraft>(
                &draft.tin,
                "0619F",
                draft.taxable_year,
                Some(draft.month),
            )
            .expect("0619-F lookup should succeed")
            .expect("saved 0619-F draft should exist");

        assert_eq!(reopened.due_day, Some(10));
        assert_eq!(reopened.due_month_and_year(), (1, 2027));
        assert_eq!(reopened.payment_details, draft.payment_details);
        assert_eq!(reopened.item_19_total_amount_of_remittance, 1_230.0);
    }

    #[test]
    fn queued_q1_eight_percent_draft_atomically_records_annual_election() {
        let db = test_db();
        let profile = test_profile();
        insert_test_profile(&db, &profile);
        let draft = queued_eight_percent_draft(&profile);

        let first_id = db.save_queued_2551q_draft_and_election(&draft).unwrap();
        assert!(
            db.save_queued_2551q_draft_and_election(&draft).is_err(),
            "an existing Queued snapshot must not be replaced by a stale requeue"
        );

        let saved_profile = db.get_profile(&draft.tin).unwrap().unwrap();
        let elections = saved_profile
            .tax_elections
            .iter()
            .filter(|entry| entry.taxable_year == 2026)
            .collect::<Vec<_>>();
        let saved_draft = db.get_2551q_draft(&draft.tin, 2026, 1).unwrap().unwrap();

        assert!(first_id > 0);
        assert_eq!(elections.len(), 1);
        assert_eq!(elections[0].election, IncomeTaxElection::EightPercent);
        assert_eq!(elections[0].source_form, "2551Qv2018");
        assert_eq!(saved_draft.status, FilingStatus::Queued);
        let saved_set = db.get_per_year_forms(&draft.tin, 2026).unwrap();
        assert!(saved_set.contains_active("1701Q"));
        assert!(saved_set.contains_active("1701"));
        assert!(!saved_set.contains_active("2551Q"));
        assert_eq!(saved_profile.per_year_forms.get(&2026), Some(&saved_set));
    }

    #[test]
    fn queued_election_reports_recorded_post_commit_refresh_request() {
        let db = test_db();
        let profile = test_profile();
        insert_test_profile(&db, &profile);
        let draft = queued_eight_percent_draft(&profile);

        let outcome = db
            .save_queued_2551q_draft_and_election_with_post_commit_status(&draft)
            .expect("the queued draft and election transaction should commit");

        assert!(outcome.refresh_status().request_recorded());
        assert!(db.get_2551q_draft(&draft.tin, 2026, 1).unwrap().is_some());
    }

    #[test]
    fn queued_election_reconciliation_preserves_manual_forms_set_decisions() {
        let db = test_db();
        let profile = test_profile();
        insert_test_profile(&db, &profile);
        let mut existing = crate::forms::PerYearFormsSet::from_codes(
            2026,
            ["2551Q", "0605"],
            crate::forms::FormSetSource::CorAi,
        );
        existing
            .entries
            .iter_mut()
            .find(|entry| entry.form_code == "2551Q")
            .expect("2551Q fixture entry")
            .apply_manual_decision(true, Some("Accountant confirmed filing".into()));
        existing
            .entries
            .iter_mut()
            .find(|entry| entry.form_code == "0605")
            .expect("0605 fixture entry")
            .apply_manual_decision(false, Some("Not applicable for this year".into()));
        db.save_per_year_forms(&profile.tin.full(), 2026, &existing)
            .unwrap();

        db.save_queued_2551q_draft_and_election(&queued_eight_percent_draft(&profile))
            .expect("profile, Forms Set, and draft should commit together");

        let saved = db.get_per_year_forms(&profile.tin.full(), 2026).unwrap();
        let included = saved.entry("2551Q").expect("manual include must remain");
        assert!(included.active);
        assert_eq!(included.source, crate::forms::FormSetSource::Manual);
        let excluded = saved.entry("0605").expect("manual exclude must remain");
        assert!(!excluded.active);
        assert_eq!(excluded.source, crate::forms::FormSetSource::Manual);
    }

    #[test]
    fn forms_set_write_failure_rolls_back_queued_election_and_draft() {
        let db = test_db();
        let profile = test_profile();
        insert_test_profile(&db, &profile);
        db.conn
            .execute_batch(
                "CREATE TRIGGER reject_item13_forms_set
                 BEFORE INSERT ON per_year_forms
                 BEGIN
                   SELECT RAISE(ABORT, 'forced Forms Set failure');
                 END;",
            )
            .unwrap();
        let draft = queued_eight_percent_draft(&profile);

        let error = db
            .save_queued_2551q_draft_and_election(&draft)
            .expect_err("a Forms Set write failure must abort the transaction");

        assert!(error.to_string().contains("forced Forms Set failure"));
        let saved_profile = db.get_profile(&draft.tin).unwrap().unwrap();
        assert!(
            saved_profile
                .tax_elections
                .iter()
                .all(|entry| entry.taxable_year != 2026)
        );
        assert!(db.get_2551q_draft(&draft.tin, 2026, 1).unwrap().is_none());
        assert!(db.get_per_year_forms(&draft.tin, 2026).unwrap().is_empty());
    }

    #[test]
    fn queued_new_registrant_initial_quarter_records_eight_percent_election() {
        let db = test_db();
        let mut profile = test_profile();
        profile.business_start_date = chrono::NaiveDate::from_ymd_opt(2026, 8, 15);
        profile.ensure_profile_version_ledger();
        insert_test_profile(&db, &profile);

        let mut draft = Form2551QDraft::new_from_effective_profile(&profile, 2026, 3);
        assert_eq!(draft.item_13_is_applicable(), Some(true));
        draft.item_13_election = Item13Election::EightPercent;
        draft
            .transition_to_queued()
            .expect("a new registrant's NIL PT010 initial return should queue");

        db.save_queued_2551q_draft_and_election(&draft)
            .expect("the draft and annual election should commit atomically");

        let saved_profile = db.get_profile(&draft.tin).unwrap().unwrap();
        assert!(saved_profile.tax_elections.iter().any(|entry| {
            entry.taxable_year == 2026 && entry.election == IncomeTaxElection::EightPercent
        }));
        assert_eq!(
            db.get_2551q_draft(&draft.tin, 2026, 3)
                .unwrap()
                .unwrap()
                .status,
            FilingStatus::Queued
        );
    }

    #[test]
    fn queued_graduated_return_atomically_records_unspecified_graduated_election() {
        let db = test_db();
        let profile = test_profile();
        insert_test_profile(&db, &profile);
        let draft = queued_graduated_draft(&profile);

        db.save_queued_2551q_draft_and_election(&draft)
            .expect("an unchanged current profile should accept the queued draft");

        let saved_profile = db.get_profile(&draft.tin).unwrap().unwrap();
        let elections = saved_profile
            .tax_elections
            .iter()
            .filter(|entry| entry.taxable_year == 2026)
            .collect::<Vec<_>>();
        assert_eq!(elections.len(), 1);
        assert_eq!(
            elections[0].election,
            IncomeTaxElection::GraduatedUnspecified
        );
        assert_eq!(elections[0].source_form, "2551Qv2018");
        assert_eq!(
            db.get_2551q_draft(&draft.tin, 2026, 1)
                .unwrap()
                .unwrap()
                .status,
            FilingStatus::Queued
        );
    }

    #[test]
    fn missing_current_profile_rejects_every_queued_election() {
        let profile = test_profile();
        for draft in [
            queued_graduated_draft(&profile),
            queued_eight_percent_draft(&profile),
        ] {
            let db = test_db();
            let error = db.save_queued_2551q_draft_and_election(&draft).unwrap_err();
            assert!(error.to_string().contains("profile"));
            assert!(db.get_2551q_draft(&draft.tin, 2026, 1).unwrap().is_none());
        }
    }

    #[test]
    fn unresolved_effective_profile_ledgers_reject_queue_without_partial_writes() {
        let reviewed_profile = test_profile();
        let cases = [
            {
                let mut profile = reviewed_profile.clone();
                profile.profile_versions.clear();
                ("missing ledger", profile)
            },
            {
                let mut profile = reviewed_profile.clone();
                profile.ensure_profile_version_ledger();
                let mut overlapping = profile.profile_versions[0].clone();
                overlapping.id = "overlapping-confirmed-version".to_string();
                overlapping.label = "Overlapping confirmed version".to_string();
                overlapping.effective_from = chrono::NaiveDate::from_ymd_opt(2025, 1, 1);
                profile.profile_versions.push(overlapping);
                ("overlapping ledger", profile)
            },
            {
                let mut profile = reviewed_profile.clone();
                profile.business_start_date = None;
                profile.profile_versions.clear();
                profile.ensure_profile_version_ledger();
                ("undated ledger", profile)
            },
            {
                let mut profile = reviewed_profile.clone();
                profile.ensure_profile_version_ledger();
                profile.profile_versions[0].effective_from =
                    chrono::NaiveDate::from_ymd_opt(2027, 1, 1);
                ("out-of-period ledger", profile)
            },
        ];

        for (case, current_profile) in cases {
            let db = test_db();
            insert_raw_test_profile(&db, &current_profile);
            let draft = queued_eight_percent_draft(&reviewed_profile);

            let error = db
                .save_queued_2551q_draft_and_election(&draft)
                .expect_err(case);

            assert!(
                error.to_string().contains("exact filing period"),
                "{case} returned an unexpected error: {error}"
            );
            assert!(db.get_2551q_draft(&draft.tin, 2026, 1).unwrap().is_none());
            assert!(
                db.get_profile(&draft.tin)
                    .unwrap()
                    .unwrap()
                    .tax_elections
                    .is_empty(),
                "{case} must not persist the draft's requested election"
            );
        }
    }

    #[test]
    fn stale_profile_derived_inputs_reject_without_partial_election_write() {
        // Taxpayer type changed after review: the stale Individual draft must
        // not append an 8% election to the now-Corporation profile.
        let db = test_db();
        let reviewed_profile = test_profile();
        let draft = queued_eight_percent_draft(&reviewed_profile);
        let mut current_profile = reviewed_profile.clone();
        current_profile.taxpayer_type = crate::profile::TaxpayerType::Corporation;
        insert_test_profile(&db, &current_profile);

        assert!(db.save_queued_2551q_draft_and_election(&draft).is_err());
        let saved_profile = db.get_profile(&draft.tin).unwrap().unwrap();
        assert!(saved_profile.tax_elections.is_empty());
        assert!(db.get_2551q_draft(&draft.tin, 2026, 1).unwrap().is_none());

        // EOPT tier is calculation-relevant and fingerprinted. A change after
        // review must likewise reject a generic graduated draft.
        let db = test_db();
        let mut reviewed_profile = test_profile();
        reviewed_profile.eopt_tier = Some(crate::profile::EoptTier::Medium);
        let draft = queued_graduated_draft(&reviewed_profile);
        let mut current_profile = reviewed_profile;
        current_profile.eopt_tier = Some(crate::profile::EoptTier::Micro);
        insert_test_profile(&db, &current_profile);

        assert!(db.save_queued_2551q_draft_and_election(&draft).is_err());
        assert!(db.get_2551q_draft(&draft.tin, 2026, 1).unwrap().is_none());
    }

    #[test]
    fn generic_2551q_persistence_cannot_create_or_replace_state() {
        let db = test_db();
        let profile = test_profile();
        let editable = Form2551QDraft::new_from_profile(&profile, 2026, 1);
        let queued = queued_graduated_draft(&profile);

        let error = db
            .save_2551q_draft(&queued)
            .expect_err("Queued state must use the dedicated reviewed queue path");

        assert!(error.to_string().contains("editable Draft"));
        for status in [FilingStatus::Draft, FilingStatus::Queued] {
            assert!(
                db.save_form_draft_v2(
                    &editable.tin,
                    "2551Q",
                    editable.taxable_year,
                    &FilingPeriod::Quarterly(editable.quarter),
                    &status,
                    &editable,
                )
                .is_err()
            );
            assert!(
                db.save_form_draft(
                    &editable.tin,
                    "2551Q",
                    editable.taxable_year,
                    Some(editable.quarter),
                    &status,
                    &editable,
                )
                .is_err()
            );
        }
        assert!(
            db.get_2551q_draft(&queued.tin, queued.taxable_year, queued.quarter)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn imported_2551q_cannot_create_or_replace_any_filing_state() {
        let db = test_db();
        let profile = test_profile();

        assert!(
            db.save_imported_form(&profile.tin.full(), "2551Q", 2035, Some(4), None)
                .is_err()
        );
        let create_count: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM form_drafts
                 WHERE tin = ?1 AND form_code = '2551Q' AND taxable_year = 2035",
                params![profile.tin.full()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(create_count, 0);

        let cases = [
            (2027, FilingStatus::Draft, false),
            (2028, FilingStatus::Queued, false),
            (2029, FilingStatus::Queued, true),
            (2030, FilingStatus::Sent, false),
            (2031, FilingStatus::Confirmed, false),
            (2032, FilingStatus::Paid, false),
        ];
        for (year, status, claimed) in cases {
            let mut stored = Form2551QDraft::new_from_profile(&profile, year, 1);
            stored.status = status.clone();
            if matches!(
                &status,
                FilingStatus::Queued
                    | FilingStatus::Sent
                    | FilingStatus::Confirmed
                    | FilingStatus::Paid
            ) {
                stored.queued_submission_fingerprint = Some(format!("reviewed-{year}-fingerprint"));
            }
            if matches!(
                &status,
                FilingStatus::Sent | FilingStatus::Confirmed | FilingStatus::Paid
            ) {
                stored.submitted_at = Some(format!("{year}-04-25T04:00:00+00:00"));
                stored.submission_filename = Some(stored.default_submission_filename());
            }
            if matches!(&status, FilingStatus::Confirmed | FilingStatus::Paid) {
                stored.confirmed_at = Some(format!("{year}-04-25T12:00:00+08:00"));
            }
            if claimed {
                stored.submission_claim_token = Some("durable-network-claim".to_string());
                stored.submission_claimed_at = Some(format!("{year}-04-25T04:00:01+00:00"));
            }
            let raw_json = serde_json::to_string(&stored).unwrap();
            db.conn
                .execute(
                    "INSERT INTO form_drafts
                        (tin, form_code, taxable_year, quarter, period_key, status, data_json)
                     VALUES (?1, '2551Q', ?2, 1, 'Q1', ?3, ?4)",
                    params![
                        &stored.tin,
                        i64::from(year),
                        filing_status_to_db(&status),
                        &raw_json
                    ],
                )
                .unwrap();
            let before = db
                .conn
                .query_row(
                    "SELECT status, data_json, updated_at FROM form_drafts
                     WHERE tin = ?1 AND form_code = '2551Q'
                       AND taxable_year = ?2 AND quarter = 1",
                    params![&stored.tin, i64::from(year)],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )
                .unwrap();

            assert!(
                db.save_imported_form(&stored.tin, "2551Q", year, Some(1), None)
                    .is_err(),
                "import must reject existing {status:?} state (claimed={claimed})"
            );
            let after = db
                .conn
                .query_row(
                    "SELECT status, data_json, updated_at FROM form_drafts
                     WHERE tin = ?1 AND form_code = '2551Q'
                       AND taxable_year = ?2 AND quarter = 1",
                    params![&stored.tin, i64::from(year)],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )
                .unwrap();
            assert_eq!(after, before);
        }
    }

    #[test]
    fn stale_editable_2551q_cannot_overwrite_a_queued_snapshot() {
        let db = test_db();
        let profile = test_profile();
        insert_test_profile(&db, &profile);
        let editable = Form2551QDraft::new_from_effective_profile(&profile, 2026, 1);
        db.save_2551q_draft(&editable).unwrap();

        let queued = queued_graduated_draft(&profile);
        db.save_queued_2551q_draft_and_election(&queued).unwrap();
        let before = serde_json::to_value(
            db.get_2551q_draft(&queued.tin, queued.taxable_year, queued.quarter)
                .unwrap()
                .unwrap(),
        )
        .unwrap();

        assert!(db.save_2551q_draft(&editable).is_err());
        assert!(
            db.save_form_draft_v2(
                &editable.tin,
                "2551Q",
                editable.taxable_year,
                &FilingPeriod::Quarterly(editable.quarter),
                &FilingStatus::Draft,
                &editable,
            )
            .is_err()
        );
        assert!(
            db.save_form_draft(
                &editable.tin,
                "2551Q",
                editable.taxable_year,
                Some(editable.quarter),
                &FilingStatus::Draft,
                &editable,
            )
            .is_err()
        );
        let after = serde_json::to_value(
            db.get_2551q_draft(&queued.tin, queued.taxable_year, queued.quarter)
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(after, before);
    }

    #[test]
    fn editable_save_and_queue_require_matching_draft_column_and_json_status() {
        let db = test_db();
        let profile = test_profile();
        insert_test_profile(&db, &profile);
        let editable = Form2551QDraft::new_from_effective_profile(&profile, 2026, 1);
        db.save_2551q_draft(&editable).unwrap();
        let queued = queued_graduated_draft(&profile);

        db.conn
            .execute(
                "UPDATE form_drafts SET status = 'Submitted'
                 WHERE tin = ?1 AND form_code = '2551Q'
                   AND taxable_year = ?2 AND quarter = ?3",
                params![
                    &editable.tin,
                    i64::from(editable.taxable_year),
                    i64::from(editable.quarter)
                ],
            )
            .unwrap();
        assert!(db.save_2551q_draft(&editable).is_err());
        assert!(db.save_queued_2551q_draft_and_election(&queued).is_err());

        let mut immutable_json = editable.clone();
        immutable_json.status = FilingStatus::Sent;
        db.conn
            .execute(
                "UPDATE form_drafts SET status = 'Draft', data_json = ?1
                 WHERE tin = ?2 AND form_code = '2551Q'
                   AND taxable_year = ?3 AND quarter = ?4",
                params![
                    serde_json::to_string(&immutable_json).unwrap(),
                    &editable.tin,
                    i64::from(editable.taxable_year),
                    i64::from(editable.quarter)
                ],
            )
            .unwrap();
        assert!(db.save_2551q_draft(&editable).is_err());
        assert!(db.save_queued_2551q_draft_and_election(&queued).is_err());
    }

    #[test]
    fn exact_unclaimed_2551q_queue_revision_accepts_retry_cas() {
        let db = test_db();
        let profile = test_profile();
        insert_test_profile(&db, &profile);
        let queued = queued_graduated_draft(&profile);
        db.save_queued_2551q_draft_and_election(&queued)
            .expect("the reviewed queue generation should persist");
        let stored = db
            .get_2551q_draft(&queued.tin, queued.taxable_year, queued.quarter)
            .unwrap()
            .unwrap();
        let expected_fingerprint = stored.queued_submission_fingerprint.clone();
        let expected_retry = stored.next_retry_at.clone();
        let expected_attempts = stored.submission_attempts;
        let mut retry = stored;
        retry.record_submission_failure("pre-network preparation failed".to_string());

        assert!(
            db.replace_unclaimed_queued_2551q_submission(
                &retry,
                &expected_fingerprint,
                &expected_retry,
                expected_attempts,
            )
            .expect("the exact queue-generation CAS should execute")
        );

        let persisted = db
            .get_2551q_draft(&queued.tin, queued.taxable_year, queued.quarter)
            .unwrap()
            .unwrap();
        assert_eq!(persisted.status, FilingStatus::Queued);
        assert_eq!(
            persisted.queued_submission_fingerprint,
            expected_fingerprint
        );
        assert_eq!(persisted.submission_attempts, 1);
        assert_eq!(persisted.next_retry_at, retry.next_retry_at);
        assert_eq!(
            persisted.last_error.as_deref(),
            Some("pre-network preparation failed")
        );
        assert!(persisted.submission_claim_token.is_none());
    }

    #[test]
    fn stale_2551q_queue_revision_cannot_overwrite_newer_generation() {
        let db = test_db();
        let profile = test_profile();
        insert_test_profile(&db, &profile);
        let queued = queued_graduated_draft(&profile);
        db.save_queued_2551q_draft_and_election(&queued)
            .expect("the reviewed queue generation should persist");
        let original = db
            .get_2551q_draft(&queued.tin, queued.taxable_year, queued.quarter)
            .unwrap()
            .unwrap();
        let original_fingerprint = original.queued_submission_fingerprint.clone();
        let original_retry = original.next_retry_at.clone();
        let original_attempts = original.submission_attempts;

        let mut newer_retry = original.clone();
        newer_retry.record_submission_failure("newer worker retry".to_string());
        assert!(
            db.replace_unclaimed_queued_2551q_submission(
                &newer_retry,
                &original_fingerprint,
                &original_retry,
                original_attempts,
            )
            .unwrap()
        );
        let before_stale_write = db
            .get_2551q_draft(&queued.tin, queued.taxable_year, queued.quarter)
            .unwrap()
            .unwrap();

        let mut stale_rejection = original;
        stale_rejection.revert_to_draft();
        assert!(
            !db.replace_unclaimed_queued_2551q_submission(
                &stale_rejection,
                &original_fingerprint,
                &original_retry,
                original_attempts,
            )
            .expect("a stale CAS should return a clean miss")
        );

        let after_stale_write = db
            .get_2551q_draft(&queued.tin, queued.taxable_year, queued.quarter)
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::to_value(after_stale_write).unwrap(),
            serde_json::to_value(before_stale_write).unwrap()
        );
    }

    #[test]
    fn orphaned_2551q_claimed_at_blocks_replacement_and_reclaim() {
        let db = test_db();
        let profile = test_profile();
        insert_test_profile(&db, &profile);
        let queued = queued_graduated_draft(&profile);
        db.save_queued_2551q_draft_and_election(&queued).unwrap();
        let stored = db
            .get_2551q_draft(&queued.tin, queued.taxable_year, queued.quarter)
            .unwrap()
            .unwrap();
        let expected_fingerprint = stored.queued_submission_fingerprint.clone();
        let expected_retry = stored.next_retry_at.clone();
        let expected_attempts = stored.submission_attempts;

        let mut claimed_replacement = stored.clone();
        claimed_replacement.submission_claimed_at = Some("2026-04-25T04:00:00+00:00".to_string());
        assert!(
            db.replace_unclaimed_queued_2551q_submission(
                &claimed_replacement,
                &expected_fingerprint,
                &expected_retry,
                expected_attempts,
            )
            .is_err(),
            "replacement input carrying only claimed_at is still claimed"
        );

        let mut orphaned = stored.clone();
        orphaned.submission_claimed_at = Some("2026-04-25T04:00:00+00:00".to_string());
        let orphaned_json = serde_json::to_string(&orphaned).unwrap();
        db.conn
            .execute(
                "UPDATE form_drafts SET data_json = ?1
                 WHERE tin = ?2 AND form_code = '2551Q'
                   AND taxable_year = ?3 AND quarter = ?4",
                params![
                    &orphaned_json,
                    &orphaned.tin,
                    i64::from(orphaned.taxable_year),
                    i64::from(orphaned.quarter)
                ],
            )
            .unwrap();

        let mut retry = orphaned.clone();
        retry.submission_claimed_at = None;
        retry.record_submission_failure("must not retry orphaned claim".to_string());
        assert!(
            !db.replace_unclaimed_queued_2551q_submission(
                &retry,
                &expected_fingerprint,
                &expected_retry,
                expected_attempts,
            )
            .unwrap(),
            "stored orphaned claimed_at must make replacement a clean CAS miss"
        );
        assert!(matches!(
            db.claim_queued_2551q_submission(
                &orphaned.tin,
                orphaned.taxable_year,
                orphaned.quarter,
                &expected_fingerprint,
                &expected_retry,
                expected_attempts,
            )
            .unwrap(),
            Claim2551QSubmissionResult::Superseded
        ));
        let after = db
            .get_2551q_draft(&orphaned.tin, orphaned.taxable_year, orphaned.quarter)
            .unwrap()
            .unwrap();
        assert_eq!(serde_json::to_string(&after).unwrap(), orphaned_json);
    }

    #[test]
    fn cancellation_uses_exact_queued_json_and_allows_invalid_unclaimed_state() {
        let db = test_db();
        let profile = test_profile();
        insert_test_profile(&db, &profile);
        let queued = queued_graduated_draft(&profile);
        db.save_queued_2551q_draft_and_election(&queued).unwrap();

        let mut changed = db
            .get_2551q_draft(&queued.tin, queued.taxable_year, queued.quarter)
            .unwrap()
            .unwrap();
        changed.profile_snapshot_stale = true;
        changed.profile_snapshot_stale_reason = Some("profile changed".to_string());
        db.conn
            .execute(
                "UPDATE form_drafts SET data_json = ?1
                 WHERE tin = ?2 AND form_code = '2551Q'
                   AND taxable_year = ?3 AND quarter = ?4",
                params![
                    serde_json::to_string(&changed).unwrap(),
                    &queued.tin,
                    i64::from(queued.taxable_year),
                    i64::from(queued.quarter)
                ],
            )
            .unwrap();

        assert!(
            db.cancel_queued_2551q_submission(&queued).is_err(),
            "a stale open view must not overwrite newer queue audit state"
        );
        let canceled = db
            .cancel_queued_2551q_submission(&changed)
            .expect("an exact unclaimed queue may be canceled even when revalidation would fail");
        assert_eq!(canceled.status, FilingStatus::Draft);
        assert!(canceled.profile_snapshot_stale);
        assert!(canceled.queued_submission_fingerprint.is_none());
    }

    #[test]
    fn exact_unclaimed_2551q_queue_revision_accepts_fifth_failure_draft_cas() {
        let db = test_db();
        let profile = test_profile();
        insert_test_profile(&db, &profile);
        let mut queued = queued_graduated_draft(&profile);
        queued.submission_attempts = 4;
        queued.next_retry_at = Some("2026-07-24T00:00:00Z".to_string());
        db.save_queued_2551q_draft_and_election(&queued)
            .expect("the fourth-retry queue generation should persist");
        let stored = db
            .get_2551q_draft(&queued.tin, queued.taxable_year, queued.quarter)
            .unwrap()
            .unwrap();
        let expected_fingerprint = stored.queued_submission_fingerprint.clone();
        let expected_retry = stored.next_retry_at.clone();
        let expected_attempts = stored.submission_attempts;
        let mut exhausted = stored;
        exhausted.record_submission_failure("fifth preparation failure".to_string());
        assert_eq!(exhausted.status, FilingStatus::Draft);

        assert!(
            db.replace_unclaimed_queued_2551q_submission(
                &exhausted,
                &expected_fingerprint,
                &expected_retry,
                expected_attempts,
            )
            .expect("the exact fifth-attempt CAS should execute")
        );

        let persisted = db
            .get_2551q_draft(&queued.tin, queued.taxable_year, queued.quarter)
            .unwrap()
            .unwrap();
        assert_eq!(persisted.status, FilingStatus::Draft);
        assert_eq!(persisted.submission_attempts, 5);
        assert!(persisted.queued_submission_fingerprint.is_none());
        assert!(persisted.next_retry_at.is_none());
        assert_eq!(
            persisted.last_error.as_deref(),
            Some("fifth preparation failure")
        );
    }

    #[test]
    fn network_claim_completion_rejects_mutation_and_finishes_exact_submission() {
        let db = test_db();
        let profile = test_profile();
        insert_test_profile(&db, &profile);
        let queued = queued_graduated_draft(&profile);
        db.save_queued_2551q_draft_and_election(&queued)
            .expect("queued draft should persist before claiming");

        let (mut claimed, token) = match db
            .claim_queued_2551q_submission(
                &queued.tin,
                queued.taxable_year,
                queued.quarter,
                &queued.queued_submission_fingerprint,
                &queued.next_retry_at,
                queued.submission_attempts,
            )
            .unwrap()
        {
            Claim2551QSubmissionResult::Claimed { draft, token } => (draft, token),
            _ => panic!("the exact queued generation should be claimed"),
        };
        assert_eq!(
            claimed.submission_claim_token.as_deref(),
            Some(token.as_str())
        );

        let mut stale_cancel = queued.clone();
        stale_cancel.revert_to_draft();
        assert!(db.save_2551q_draft(&stale_cancel).is_err());
        let still_claimed = db
            .get_2551q_draft(&queued.tin, queued.taxable_year, queued.quarter)
            .unwrap()
            .unwrap();
        assert_eq!(
            still_claimed.submission_claim_token.as_deref(),
            Some(token.as_str())
        );

        let mut arbitrary_clear = claimed.clone();
        arbitrary_clear.submission_claim_token = None;
        arbitrary_clear.submission_claimed_at = None;
        assert!(
            db.finish_claimed_2551q_submission(&arbitrary_clear, &token)
                .is_err(),
            "a worker may not clear an unknown-outcome claim without a terminal or retry transition"
        );

        let expected_filename = claimed.default_submission_filename();
        let mut wrong_status = claimed.clone();
        wrong_status.submission_claim_token = None;
        wrong_status.submission_claimed_at = None;
        wrong_status.status = FilingStatus::Confirmed;
        assert!(
            db.finish_claimed_2551q_submission(&wrong_status, &token)
                .is_err()
        );

        let mut mutated = claimed.clone();
        mutated.transition_to_submitted(expected_filename.clone());
        mutated.schedule_1[0].taxable_amount += 1.0;
        assert!(
            db.finish_claimed_2551q_submission(&mutated, &token)
                .is_err()
        );

        let mut missing_metadata = claimed.clone();
        missing_metadata.transition_to_submitted(expected_filename.clone());
        missing_metadata.submitted_at = None;
        assert!(
            db.finish_claimed_2551q_submission(&missing_metadata, &token)
                .is_err()
        );

        let mut changed_fingerprint = claimed.clone();
        changed_fingerprint.transition_to_submitted(expected_filename.clone());
        changed_fingerprint.queued_submission_fingerprint = Some("different".to_string());
        assert!(
            db.finish_claimed_2551q_submission(&changed_fingerprint, &token)
                .is_err()
        );

        let mut wrong_filename = claimed.clone();
        wrong_filename.transition_to_submitted("wrong.xml".to_string());
        assert!(
            db.finish_claimed_2551q_submission(&wrong_filename, &token)
                .is_err()
        );

        claimed.transition_to_submitted(expected_filename.clone());
        assert!(
            db.finish_claimed_2551q_submission(&claimed, "wrong-token")
                .is_err()
        );
        db.finish_claimed_2551q_submission(&claimed, &token)
            .expect("only the claim owner may finish the exact transmitted snapshot");
        let submitted = db
            .get_2551q_draft(&queued.tin, queued.taxable_year, queued.quarter)
            .unwrap()
            .unwrap();
        assert_eq!(submitted.status, FilingStatus::Sent);
        assert_eq!(
            submitted.submission_filename.as_deref(),
            Some(expected_filename.as_str())
        );
        assert_eq!(
            submitted.queued_submission_fingerprint,
            queued.queued_submission_fingerprint
        );
        assert!(submitted.submission_claim_token.is_none());
        assert!(submitted.submission_claimed_at.is_none());
    }

    #[test]
    fn network_claim_completion_rejects_retry_and_exhaustion_states() {
        let db = test_db();
        let profile = test_profile();
        insert_test_profile(&db, &profile);
        let queued = queued_graduated_draft(&profile);
        db.save_queued_2551q_draft_and_election(&queued)
            .expect("queued draft should persist before claiming");

        let (mut claimed, token) = match db
            .claim_queued_2551q_submission(
                &queued.tin,
                queued.taxable_year,
                queued.quarter,
                &queued.queued_submission_fingerprint,
                &queued.next_retry_at,
                queued.submission_attempts,
            )
            .unwrap()
        {
            Claim2551QSubmissionResult::Claimed { draft, token } => (draft, token),
            _ => panic!("the exact queued generation should be claimed"),
        };
        claimed.record_submission_failure("definitely pre-transmission".to_string());
        assert!(
            db.finish_claimed_2551q_submission(&claimed, &token)
                .is_err(),
            "post-claim transport failure has an unknown outcome and must not schedule a retry"
        );

        let still_claimed = db
            .get_2551q_draft(&queued.tin, queued.taxable_year, queued.quarter)
            .unwrap()
            .unwrap();
        assert_eq!(still_claimed.status, FilingStatus::Queued);
        assert_eq!(
            still_claimed.submission_attempts,
            queued.submission_attempts
        );
        assert_eq!(
            still_claimed.queued_submission_fingerprint,
            queued.queued_submission_fingerprint
        );
        assert_eq!(
            still_claimed.submission_claim_token.as_deref(),
            Some(token.as_str())
        );
        assert!(still_claimed.submission_claimed_at.is_some());
        assert!(
            still_claimed
                .last_error
                .as_deref()
                .is_some_and(|message| message.contains("outcome pending"))
        );

        let db = test_db();
        let profile = test_profile();
        insert_test_profile(&db, &profile);
        let mut queued = queued_graduated_draft(&profile);
        queued.submission_attempts = 4;
        queued.next_retry_at = Some("2026-07-24T00:00:00Z".to_string());
        db.save_queued_2551q_draft_and_election(&queued).unwrap();
        let (mut claimed, token) = match db
            .claim_queued_2551q_submission(
                &queued.tin,
                queued.taxable_year,
                queued.quarter,
                &queued.queued_submission_fingerprint,
                &queued.next_retry_at,
                queued.submission_attempts,
            )
            .unwrap()
        {
            Claim2551QSubmissionResult::Claimed { draft, token } => (draft, token),
            _ => panic!("the exact fourth-retry generation should be claimed"),
        };
        claimed.record_submission_failure("fifth failure after claim".to_string());
        assert_eq!(claimed.status, FilingStatus::Draft);
        assert!(
            db.finish_claimed_2551q_submission(&claimed, &token)
                .is_err(),
            "retry exhaustion after claim must not clear an unknown-outcome claim"
        );
        let still_claimed = db
            .get_2551q_draft(&queued.tin, queued.taxable_year, queued.quarter)
            .unwrap()
            .unwrap();
        assert_eq!(still_claimed.status, FilingStatus::Queued);
        assert_eq!(still_claimed.submission_attempts, 4);
        assert_eq!(
            still_claimed.submission_claim_token.as_deref(),
            Some(token.as_str())
        );
        assert!(still_claimed.submission_claimed_at.is_some());
    }

    #[test]
    fn receipt_confirmation_accepts_audited_alias_binds_time_and_preserves_submission_filename() {
        let db = test_db();
        let submitted = submitted_2551q_draft(&db, &test_profile());
        let submitted_filename = submitted.submission_filename.clone().unwrap();
        let receipt_filename = submitted_filename
            .replace("#test@example.com#", "")
            .replace("2551Qv2018", "2551Q");
        let receipt_id =
            insert_2551q_receipt(&db, &submitted, &receipt_filename, "2026-04-25", "12:00:00");
        db.conn
            .execute(
                "UPDATE submission_receipts SET form_type = '2551Q' WHERE id = ?1",
                params![receipt_id],
            )
            .unwrap();

        let mut confirmation = submitted.clone();
        confirmation.transition_to_confirmed(
            "2099-01-01T00:00:00Z".to_string(),
            Some(receipt_id),
            Some(receipt_filename),
        );
        db.save_confirmed_2551q_draft(&confirmation)
            .expect("the exact stripped receipt filename should confirm the submission");

        let persisted = db
            .get_2551q_draft(&submitted.tin, submitted.taxable_year, submitted.quarter)
            .unwrap()
            .unwrap();
        assert_eq!(persisted.status, FilingStatus::Confirmed);
        assert_eq!(persisted.receipt_id, Some(receipt_id));
        assert_eq!(
            persisted.confirmed_at.as_deref(),
            Some("2026-04-25T12:00:00+08:00")
        );
        assert_eq!(
            persisted.submission_filename.as_deref(),
            Some(submitted_filename.as_str())
        );
    }

    #[test]
    fn email_poll_confirms_a_2551q_whose_bir_stamp_precedes_the_local_submitted_time() {
        // Live 22 October 2026: BIR stamps the file when the SFTP PUT lands
        // and prints only the minute ("11:58 AM"), while the row turns
        // Submitted after PUT returns (11:58:xx). Comparing the receipt to
        // `submitted_at` ignored both confirmations on every poll, so the
        // watcher jobs never finished. The floor is the queue time, as 1601C.
        let db = test_db();
        let submitted = submitted_2551q_draft(&db, &test_profile());
        let receipt_filename = submitted
            .submission_filename
            .clone()
            .unwrap()
            .replace("#test@example.com#", "");

        // Older than this queued generation (11:55 PHT, minus skew): ignored.
        let receipt_id =
            insert_2551q_receipt(&db, &submitted, &receipt_filename, "2026-04-25", "11:50:00");
        let stale = db
            .get_submission_receipt_by_id(receipt_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            db.confirm_2551q_from_receipt(&stale).unwrap(),
            crate::db::ReceiptConfirmationOutcome::Ignored
        );

        // Same generation, stamped a minute before local Submitted: confirms.
        db.conn
            .execute(
                "UPDATE submission_receipts SET received_time = '11:59:00' WHERE id = ?1",
                params![receipt_id],
            )
            .unwrap();
        let receipt = db
            .get_submission_receipt_by_id(receipt_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            db.confirm_2551q_from_receipt(&receipt).unwrap(),
            crate::db::ReceiptConfirmationOutcome::Confirmed
        );
        let persisted = db
            .get_2551q_draft(&submitted.tin, submitted.taxable_year, submitted.quarter)
            .unwrap()
            .unwrap();
        assert_eq!(persisted.status, FilingStatus::Confirmed);
        assert_eq!(persisted.receipt_id, Some(receipt_id));
        assert_eq!(
            persisted.confirmed_at.as_deref(),
            Some("2026-04-25T11:59:00+08:00")
        );
    }

    #[test]
    fn unpersisted_receipt_cannot_fall_through_to_manual_confirmation() {
        let db = test_db();
        let submitted = submitted_2551q_draft(&db, &test_profile());
        let filename = submitted
            .submission_filename
            .as_deref()
            .unwrap()
            .replace("#test@example.com#", "");
        let receipt = crate::db::SubmissionReceipt {
            id: None,
            filename,
            tin: submitted.tin.clone(),
            form_type: AUDITED_2551Q_RECEIPT_FORM.to_string(),
            period: submitted.period_code(),
            received_date: "2026-04-25".to_string(),
            received_time: "12:00:00".to_string(),
            source_from: None,
            raw_text: "unpersisted receipt".to_string(),
            raw_html: None,
            email_received_at: None,
            created_at: None,
        };

        assert!(db.confirm_2551q_from_receipt(&receipt).is_err());
        let persisted = db
            .get_2551q_draft(&submitted.tin, submitted.taxable_year, submitted.quarter)
            .unwrap()
            .unwrap();
        assert_eq!(persisted.status, FilingStatus::Sent);
        assert!(persisted.confirmed_at.is_none());
        assert!(persisted.receipt_id.is_none());
    }

    #[test]
    fn receipt_confirmation_rejects_missing_mismatched_old_and_invalid_receipts() {
        let db = test_db();
        let submitted = submitted_2551q_draft(&db, &test_profile());
        let submitted_filename = submitted.submission_filename.clone().unwrap();
        let receipt_filename = submitted_filename.replace("#test@example.com#", "");

        let mut missing = submitted.clone();
        missing.transition_to_confirmed(
            "2099-01-01T00:00:00Z".to_string(),
            Some(999_999),
            Some(receipt_filename.clone()),
        );
        assert_2551q_confirmation_rejected_without_draft_mutation(&db, &missing);

        let receipt_id =
            insert_2551q_receipt(&db, &submitted, &receipt_filename, "2026-04-25", "12:00:00");
        let mut confirmation = submitted.clone();
        confirmation.transition_to_confirmed(
            "2099-01-01T00:00:00Z".to_string(),
            Some(receipt_id),
            Some(receipt_filename.clone()),
        );

        let mut arbitrary_caller_filename = confirmation.clone();
        arbitrary_caller_filename.submission_filename =
            Some("123456789000-2551Qv2018-122026Q1-unreviewed-copy.xml".to_string());
        assert_2551q_confirmation_rejected_without_draft_mutation(&db, &arbitrary_caller_filename);

        let expected_period = submitted.period_code();
        let receipt_filename_without_extension = receipt_filename
            .strip_suffix(".xml")
            .expect("test filename");
        let mismatches = [
            ("tin", "000000000000", submitted.tin.as_str()),
            ("form_type", "2550Q", AUDITED_2551Q_RECEIPT_FORM),
            ("period", "122026Q2", expected_period.as_str()),
            (
                "filename",
                "123456789000-2551Qv2018-122026Q1-copy.xml",
                receipt_filename.as_str(),
            ),
            (
                "filename",
                receipt_filename_without_extension,
                receipt_filename.as_str(),
            ),
        ];
        for (column, invalid, valid) in mismatches {
            db.conn
                .execute(
                    &format!("UPDATE submission_receipts SET {column} = ?1 WHERE id = ?2"),
                    params![invalid, receipt_id],
                )
                .unwrap();
            assert_2551q_confirmation_rejected_without_draft_mutation(&db, &confirmation);
            db.conn
                .execute(
                    &format!("UPDATE submission_receipts SET {column} = ?1 WHERE id = ?2"),
                    params![valid, receipt_id],
                )
                .unwrap();
        }

        db.conn
            .execute(
                "UPDATE submission_receipts
                 SET received_time = '11:52:59'
                 WHERE id = ?1",
                params![receipt_id],
            )
            .unwrap();
        assert_2551q_confirmation_rejected_without_draft_mutation(&db, &confirmation);
        db.conn
            .execute(
                "UPDATE submission_receipts
                 SET received_date = 'not-a-date', received_time = '12:00:00'
                 WHERE id = ?1",
                params![receipt_id],
            )
            .unwrap();
        assert_2551q_confirmation_rejected_without_draft_mutation(&db, &confirmation);
    }

    #[test]
    fn manual_2551q_confirmation_remains_an_explicit_receiptless_path() {
        let db = test_db();
        let submitted = submitted_2551q_draft(&db, &test_profile());
        let mut stale_confirmation = submitted.clone();
        stale_confirmation.transition_to_confirmed(
            "2026-04-25T03:59:59+00:00".to_string(),
            None,
            None,
        );
        assert_2551q_confirmation_rejected_without_draft_mutation(&db, &stale_confirmation);

        let mut confirmation = submitted.clone();
        confirmation.transition_to_confirmed("2026-04-25T12:30:00+08:00".to_string(), None, None);

        db.save_confirmed_2551q_draft(&confirmation)
            .expect("manual confirmation should remain available without a receipt");
        let persisted = db
            .get_2551q_draft(&submitted.tin, submitted.taxable_year, submitted.quarter)
            .unwrap()
            .unwrap();
        assert_eq!(persisted.status, FilingStatus::Confirmed);
        assert_eq!(persisted.receipt_id, None);
        assert_eq!(
            persisted.confirmed_at.as_deref(),
            Some("2026-04-25T12:30:00+08:00")
        );
        assert_eq!(persisted.submission_filename, submitted.submission_filename);
    }

    #[test]
    fn stale_2551q_requeue_cannot_resurrect_submitted_confirmed_or_paid_state() {
        let db = test_db();
        let profile = test_profile();
        insert_test_profile(&db, &profile);
        let stale_queue = queued_graduated_draft(&profile);
        db.save_queued_2551q_draft_and_election(&stale_queue)
            .expect("initial queue insertion should succeed");

        let (mut claimed, token) = match db
            .claim_queued_2551q_submission(
                &stale_queue.tin,
                stale_queue.taxable_year,
                stale_queue.quarter,
                &stale_queue.queued_submission_fingerprint,
                &stale_queue.next_retry_at,
                stale_queue.submission_attempts,
            )
            .unwrap()
        {
            Claim2551QSubmissionResult::Claimed { draft, token } => (draft, token),
            _ => panic!("the exact queued generation should be claimed"),
        };
        let filename = claimed.default_submission_filename();
        claimed.transition_to_submitted(filename);
        db.finish_claimed_2551q_submission(&claimed, &token)
            .unwrap();
        assert!(
            db.save_queued_2551q_draft_and_election(&stale_queue)
                .is_err(),
            "Submitted must not be resurrected as Queued"
        );

        let mut confirmed = db
            .get_2551q_draft(
                &stale_queue.tin,
                stale_queue.taxable_year,
                stale_queue.quarter,
            )
            .unwrap()
            .unwrap();
        let confirmed_at = confirmed.submitted_at.clone().unwrap();
        confirmed.transition_to_confirmed(confirmed_at, None, None);
        db.save_confirmed_2551q_draft(&confirmed).unwrap();
        assert!(
            db.save_queued_2551q_draft_and_election(&stale_queue)
                .is_err(),
            "Confirmed must not be resurrected as Queued"
        );

        confirmed.transition_to_paid();
        db.save_paid_2551q_draft(&confirmed).unwrap();
        assert!(
            db.save_queued_2551q_draft_and_election(&stale_queue)
                .is_err(),
            "Paid must not be resurrected as Queued"
        );
        assert_eq!(
            db.get_2551q_draft(
                &stale_queue.tin,
                stale_queue.taxable_year,
                stale_queue.quarter,
            )
            .unwrap()
            .unwrap()
            .status,
            FilingStatus::Paid
        );
    }

    #[test]
    fn immutable_profile_reconciliation_preserves_newer_filing_state_and_payload() {
        let db = test_db();
        let profile = test_profile();
        insert_test_profile(&db, &profile);
        let stale_open_view = queued_graduated_draft(&profile);
        db.save_queued_2551q_draft_and_election(&stale_open_view)
            .expect("queued draft should persist before claiming");

        let (mut claimed, token) = match db
            .claim_queued_2551q_submission(
                &stale_open_view.tin,
                stale_open_view.taxable_year,
                stale_open_view.quarter,
                &stale_open_view.queued_submission_fingerprint,
                &stale_open_view.next_retry_at,
                stale_open_view.submission_attempts,
            )
            .unwrap()
        {
            Claim2551QSubmissionResult::Claimed { draft, token } => (draft, token),
            _ => panic!("the exact queued generation should be claimed"),
        };
        let submission_filename = claimed.default_submission_filename();
        claimed.transition_to_submitted(submission_filename.clone());
        db.finish_claimed_2551q_submission(&claimed, &token)
            .expect("the claim owner should advance the stored return");

        let mut changed_profile = profile.clone();
        changed_profile.ensure_profile_version_ledger();
        changed_profile.full_name = "Changed Taxpayer Name".to_string();
        changed_profile.profile_versions[0].cor.registered_name =
            "Changed Taxpayer Name".to_string();

        // The caller still owns the pre-claim Queued snapshot, but the narrow
        // reconciliation path reloads the authoritative Submitted row.
        assert_eq!(stale_open_view.status, FilingStatus::Queued);
        let submitted = db
            .reconcile_immutable_2551q_profile_snapshot(
                &stale_open_view.tin,
                stale_open_view.taxable_year,
                stale_open_view.quarter,
                &changed_profile,
            )
            .expect("immutable marker reconciliation should succeed");
        assert_eq!(submitted.status, FilingStatus::Sent);
        assert_eq!(
            submitted.submission_filename.as_deref(),
            Some(submission_filename.as_str())
        );
        assert_eq!(submitted.taxpayer_name, "Test Taxpayer");
        assert!(submitted.profile_snapshot_stale);

        let mut confirmed = submitted.clone();
        let confirmed_at = confirmed.submitted_at.clone().unwrap();
        confirmed.transition_to_confirmed(confirmed_at, None, None);
        db.save_confirmed_2551q_draft(&confirmed)
            .expect("confirmation transition should persist");
        let confirmed = db
            .reconcile_immutable_2551q_profile_snapshot(
                &stale_open_view.tin,
                stale_open_view.taxable_year,
                stale_open_view.quarter,
                &changed_profile,
            )
            .expect("confirmed marker reconciliation should preserve confirmation");
        assert_eq!(confirmed.status, FilingStatus::Confirmed);
        assert_eq!(confirmed.receipt_id, None);
        assert_eq!(
            confirmed.submission_filename.as_deref(),
            Some(submission_filename.as_str())
        );

        let mut paid = confirmed;
        paid.transition_to_paid();
        db.save_paid_2551q_draft(&paid)
            .expect("payment transition should persist");
        let paid = db
            .reconcile_immutable_2551q_profile_snapshot(
                &stale_open_view.tin,
                stale_open_view.taxable_year,
                stale_open_view.quarter,
                &changed_profile,
            )
            .expect("paid marker reconciliation should preserve payment");
        assert_eq!(paid.status, FilingStatus::Paid);
        assert_eq!(paid.receipt_id, None);
        assert_eq!(paid.taxpayer_name, "Test Taxpayer");

        let stored = db
            .get_2551q_draft(
                &stale_open_view.tin,
                stale_open_view.taxable_year,
                stale_open_view.quarter,
            )
            .unwrap()
            .unwrap();
        assert_eq!(stored.status, FilingStatus::Paid);
        assert_eq!(stored.receipt_id, None);
        assert_eq!(stored.taxpayer_name, "Test Taxpayer");
        assert!(stored.profile_snapshot_stale);
    }

    #[test]
    fn immutable_profile_reconciliation_refuses_editable_draft_rows() {
        let db = test_db();
        let mut profile = test_profile();
        profile.ensure_profile_version_ledger();
        let draft = Form2551QDraft::new_from_effective_profile(&profile, 2026, 1);
        db.save_2551q_draft(&draft)
            .expect("editable draft should use the generic draft save path");
        let before = serde_json::to_value(
            db.get_2551q_draft(&draft.tin, draft.taxable_year, draft.quarter)
                .unwrap()
                .unwrap(),
        )
        .unwrap();

        let error = db
            .reconcile_immutable_2551q_profile_snapshot(
                &draft.tin,
                draft.taxable_year,
                draft.quarter,
                &profile,
            )
            .expect_err("immutable reconciliation must reject an editable draft");
        assert!(error.to_string().contains("Editable 2551Q drafts"));

        let after = serde_json::to_value(
            db.get_2551q_draft(&draft.tin, draft.taxable_year, draft.quarter)
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(after, before);
    }

    #[test]
    fn abandoned_network_claim_stays_fail_closed_for_manual_reconciliation() {
        let db = test_db();
        let profile = test_profile();
        insert_test_profile(&db, &profile);
        let queued = queued_graduated_draft(&profile);
        db.save_queued_2551q_draft_and_election(&queued)
            .expect("queued draft should persist before claiming");

        let token = match db
            .claim_queued_2551q_submission(
                &queued.tin,
                queued.taxable_year,
                queued.quarter,
                &queued.queued_submission_fingerprint,
                &queued.next_retry_at,
                queued.submission_attempts,
            )
            .unwrap()
        {
            Claim2551QSubmissionResult::Claimed { token, .. } => token,
            _ => panic!("the exact queued generation should be claimed"),
        };

        // Simulate a process crash by abandoning the claim without calling the
        // completion method. There is no time-based reclamation because the
        // transport outcome is unknowable and an automatic retry could duplicate
        // a filing that BIR already received.
        let abandoned = db
            .get_2551q_draft(&queued.tin, queued.taxable_year, queued.quarter)
            .unwrap()
            .unwrap();
        assert_eq!(abandoned.status, FilingStatus::Queued);
        assert_eq!(
            abandoned.submission_claim_token.as_deref(),
            Some(token.as_str())
        );
        assert!(abandoned.submission_claimed_at.is_some());
        assert!(
            abandoned
                .last_error
                .as_deref()
                .is_some_and(|message| message.contains("Automatic retry is disabled"))
        );
        assert!(
            abandoned
                .last_error
                .as_deref()
                .is_some_and(|message| message.contains("contact support"))
        );
        assert!(
            db.reconcile_immutable_2551q_profile_snapshot(
                &queued.tin,
                queued.taxable_year,
                queued.quarter,
                &profile,
            )
            .is_err(),
            "profile reconciliation must not mutate the exact claimed JSON snapshot"
        );

        assert!(matches!(
            db.claim_queued_2551q_submission(
                &queued.tin,
                queued.taxable_year,
                queued.quarter,
                &queued.queued_submission_fingerprint,
                &queued.next_retry_at,
                queued.submission_attempts,
            )
            .unwrap(),
            Claim2551QSubmissionResult::Superseded
        ));

        assert!(
            db.save_queued_2551q_draft_and_election(&queued).is_err(),
            "a claimed queue snapshot must not be replaced through requeue"
        );
        let mut stale_cancel = queued;
        stale_cancel.revert_to_draft();
        assert!(db.save_2551q_draft(&stale_cancel).is_err());
    }

    #[test]
    fn claimed_2551q_cannot_cancel_but_human_confirmed_release_returns_draft() {
        let db = test_db();
        let profile = test_profile();
        insert_test_profile(&db, &profile);
        let queued = queued_graduated_draft(&profile);
        db.save_queued_2551q_draft_and_election(&queued)
            .expect("queued draft should persist before claiming");
        let (claimed, token) = match db
            .claim_queued_2551q_submission(
                &queued.tin,
                queued.taxable_year,
                queued.quarter,
                &queued.queued_submission_fingerprint,
                &queued.next_retry_at,
                queued.submission_attempts,
            )
            .unwrap()
        {
            Claim2551QSubmissionResult::Claimed { draft, token } => (draft, token),
            _ => panic!("queued 2551Q should be claimed"),
        };
        assert!(db.cancel_queued_2551q_submission(&claimed).is_err());

        let released = db
            .release_abandoned_claimed_2551q_submission(
                &claimed.tin,
                claimed.taxable_year,
                claimed.quarter,
                ABANDONED_CLAIM_RELEASE_REASON,
            )
            .unwrap();
        let AbandonedClaimRelease::Released { draft, .. } = released else {
            panic!("claimed 2551Q should release to Draft");
        };
        assert_eq!(draft.status, FilingStatus::Draft);
        assert!(draft.submission_claim_token.is_none());
        assert!(draft.submission_claimed_at.is_none());
        assert!(
            draft
                .last_error
                .as_deref()
                .is_some_and(|message| message.contains("Abandoned claim released"))
        );
        assert!(
            !draft
                .last_error
                .as_deref()
                .is_some_and(|message| message.contains("Automatic retry is disabled"))
        );

        let mut finished = claimed.clone();
        finished.transition_to_submitted(claimed.default_submission_filename());
        assert!(
            db.finish_claimed_2551q_submission(&finished, &token)
                .is_err()
        );
    }

    #[test]
    fn network_claim_is_a_queue_revision_cas_and_revalidates_profile() {
        let db = test_db();
        let mut profile = test_profile();
        profile.eopt_tier = Some(crate::profile::EoptTier::Medium);
        profile.ensure_profile_version_ledger();
        insert_test_profile(&db, &profile);
        let queued = queued_graduated_draft(&profile);
        db.save_queued_2551q_draft_and_election(&queued)
            .expect("queued draft should persist before claiming");

        assert!(matches!(
            db.claim_queued_2551q_submission(
                &queued.tin,
                queued.taxable_year,
                queued.quarter,
                &queued.queued_submission_fingerprint,
                &Some("different queue generation".to_string()),
                queued.submission_attempts,
            )
            .unwrap(),
            Claim2551QSubmissionResult::Superseded
        ));

        profile.eopt_tier = Some(crate::profile::EoptTier::Micro);
        profile.profile_versions[0].eopt_tier = Some(crate::profile::EoptTier::Micro);
        db.conn
            .execute(
                "UPDATE profiles SET data_json = ?1 WHERE tin = ?2",
                params![serde_json::to_string(&profile).unwrap(), &queued.tin],
            )
            .unwrap();
        match db
            .claim_queued_2551q_submission(
                &queued.tin,
                queued.taxable_year,
                queued.quarter,
                &queued.queued_submission_fingerprint,
                &queued.next_retry_at,
                queued.submission_attempts,
            )
            .unwrap()
        {
            Claim2551QSubmissionResult::Rejected { draft, errors } => {
                assert_eq!(draft.status, FilingStatus::Draft);
                assert!(errors.iter().any(|(field, _)| field == "profile_snapshot"));
            }
            _ => panic!("a changed current profile must not be claimed"),
        }
        let stored = db
            .get_2551q_draft(&queued.tin, queued.taxable_year, queued.quarter)
            .unwrap()
            .unwrap();
        assert_eq!(stored.status, FilingStatus::Draft);
        assert!(stored.queued_submission_fingerprint.is_none());
        assert!(stored.submission_claim_token.is_none());
    }

    #[test]
    fn network_claim_rejects_when_the_effective_profile_becomes_unresolved() {
        let db = test_db();
        let profile = test_profile();
        insert_test_profile(&db, &profile);
        let queued = queued_graduated_draft(&profile);
        db.save_queued_2551q_draft_and_election(&queued)
            .expect("queued draft should persist before claiming");

        let mut unresolved_profile = db.get_profile(&queued.tin).unwrap().unwrap();
        unresolved_profile.profile_versions.clear();
        db.conn
            .execute(
                "UPDATE profiles SET data_json = ?1 WHERE tin = ?2",
                params![
                    serde_json::to_string(&unresolved_profile).unwrap(),
                    &queued.tin
                ],
            )
            .unwrap();

        match db
            .claim_queued_2551q_submission(
                &queued.tin,
                queued.taxable_year,
                queued.quarter,
                &queued.queued_submission_fingerprint,
                &queued.next_retry_at,
                queued.submission_attempts,
            )
            .unwrap()
        {
            Claim2551QSubmissionResult::Rejected { draft, errors } => {
                assert_eq!(draft.status, FilingStatus::Draft);
                assert!(
                    draft
                        .last_error
                        .as_deref()
                        .is_some_and(|message| message.contains("effective taxpayer profile"))
                );
                assert!(errors.iter().any(|(field, message)| {
                    field == "profile_resolution" && message.contains("No confirmed")
                }));
            }
            _ => panic!("an unresolved profile must reject a network claim"),
        }

        let stored = db
            .get_2551q_draft(&queued.tin, queued.taxable_year, queued.quarter)
            .unwrap()
            .unwrap();
        assert_eq!(stored.status, FilingStatus::Draft);
        assert!(stored.queued_submission_fingerprint.is_none());
        assert!(stored.submission_claim_token.is_none());
        assert!(
            stored
                .last_error
                .as_deref()
                .is_some_and(|message| message.contains("effective taxpayer profile"))
        );
    }

    #[test]
    fn conflicting_annual_election_rejects_queued_draft_without_changes() {
        let db = test_db();
        let mut profile = test_profile();
        profile.tax_elections.push(TaxElectionHistory {
            taxable_year: 2026,
            election: IncomeTaxElection::GraduatedOsd,
            elected_at: chrono::Utc::now().naive_utc(),
            source_form: "1701Q".to_string(),
        });
        insert_test_profile(&db, &profile);
        let draft = queued_eight_percent_draft(&test_profile());

        let error = db.save_queued_2551q_draft_and_election(&draft).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("conflicting income-tax election")
        );
        assert!(db.get_2551q_draft(&draft.tin, 2026, 1).unwrap().is_none());
        let saved_profile = db.get_profile(&draft.tin).unwrap().unwrap();
        assert_eq!(saved_profile.tax_elections.len(), 1);
        assert_eq!(
            saved_profile.tax_elections[0].election,
            IncomeTaxElection::GraduatedOsd
        );
    }

    #[test]
    fn draft_write_failure_rolls_back_new_profile_election() {
        let db = test_db();
        let profile = test_profile();
        insert_test_profile(&db, &profile);
        db.conn
            .execute_batch(
                "CREATE TRIGGER reject_2551q_draft
                 BEFORE INSERT ON form_drafts
                 WHEN NEW.form_code = '2551Q'
                 BEGIN
                     SELECT RAISE(ABORT, 'forced draft failure');
                 END;",
            )
            .unwrap();
        let draft = queued_eight_percent_draft(&profile);

        assert!(db.save_queued_2551q_draft_and_election(&draft).is_err());
        assert!(db.get_2551q_draft(&draft.tin, 2026, 1).unwrap().is_none());
        let saved_profile = db.get_profile(&draft.tin).unwrap().unwrap();
        assert!(saved_profile.tax_elections.is_empty());
    }

    #[test]
    fn period_key_upsert_updates_annual_draft_in_place() {
        let db = test_db();
        let period = FilingPeriod::Annual;
        let first_id = db
            .save_form_draft_v2(
                "123456789000",
                "1702MX",
                2026,
                &period,
                &FilingStatus::Draft,
                &TestDraft { value: 1 },
            )
            .unwrap();
        let second_id = db
            .save_form_draft_v2(
                "123456789000",
                "1702MX",
                2026,
                &period,
                &FilingStatus::Draft,
                &TestDraft { value: 2 },
            )
            .unwrap();

        let count: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM form_drafts
                 WHERE tin = '123456789000' AND form_code = '1702MX' AND taxable_year = 2026",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let loaded: TestDraft = db
            .get_form_draft_v2("123456789000", "1702MX", 2026, &period)
            .unwrap()
            .unwrap();

        assert_eq!(first_id, second_id);
        assert_eq!(count, 1);
        assert_eq!(loaded, TestDraft { value: 2 });
    }

    #[test]
    fn scaffold_forms_reject_queue_persistence() {
        let db = test_db();
        // Use a form that is still ExternalOrManualOnly (1700 is not implemented in-app)
        let result = db.save_form_draft_v2(
            "123456789000",
            "1700",
            2026,
            &FilingPeriod::Annual,
            &FilingStatus::Queued,
            &TestDraft { value: 1 },
        );

        assert!(result.is_err());
        let summaries = db.list_draft_summaries("123456789000", 2026).unwrap();
        assert!(summaries.is_empty());
        assert!(db.list_all_queued_submissions().unwrap().is_empty());
    }

    #[test]
    fn generic_draft_persistence_cannot_queue_a_transport_capable_form() {
        let db = test_db();
        let result = db.save_form_draft_v2(
            "123456789000",
            "2551Q",
            2026,
            &FilingPeriod::Quarterly(1),
            &FilingStatus::Queued,
            &TestDraft { value: 1 },
        );

        assert!(result.is_err());
        let persisted: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM form_drafts
                 WHERE tin = '123456789000'
                   AND form_code = '2551Q'
                   AND taxable_year = 2026
                   AND status = 'Queued'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(persisted, 0);
    }

    #[test]
    fn annual_and_open_ended_progress_use_period_key_semantics() {
        let db = test_db();
        db.save_form_draft_v2(
            "123456789000",
            "1701",
            2026,
            &FilingPeriod::Annual,
            &FilingStatus::Sent,
            &TestDraft { value: 1 },
        )
        .unwrap();
        db.save_form_draft_v2(
            "123456789000",
            "0605",
            2026,
            &FilingPeriod::OpenEnded(1),
            &FilingStatus::Sent,
            &TestDraft { value: 1 },
        )
        .unwrap();
        db.save_form_draft_v2(
            "123456789000",
            "0605",
            2026,
            &FilingPeriod::OpenEnded(2),
            &FilingStatus::Draft,
            &TestDraft { value: 2 },
        )
        .unwrap();

        let annual = db
            .get_form_filing_progress("123456789000", "1701", 2026)
            .unwrap();
        let open_ended = db
            .get_form_filing_progress("123456789000", "0605", 2026)
            .unwrap();

        assert_eq!(annual.annual_status, QuarterState::Sent);
        assert_eq!(open_ended.open_ended_count, 1);
    }

    #[test]
    fn quarterly_2550q_submitted_shows_on_its_quarter_tile() {
        let db = test_db();
        let tin = "123456789000";
        // Stored the way the queue path writes a 2550Q row: legacy quarter
        // column plus the period key, then moved to Submitted by the worker.
        let key = FilingPeriod::Quarterly(3).to_period_key();
        db.conn
            .execute(
                "INSERT INTO form_drafts
                    (tin, form_code, taxable_year, quarter, period_key, status, data_json)
                 VALUES (?1, '2550Q', 2026, 3, ?2, 'Queued', '{}')",
                params![tin, key],
            )
            .unwrap();
        db.conn
            .execute(
                "UPDATE form_drafts SET status = 'Submitted'
                 WHERE tin = ?1 AND form_code = '2550Q' AND quarter = 3",
                params![tin],
            )
            .unwrap();
        // A Draft for Q1, and a stray Q3 Draft row with no legacy quarter
        // (the unique index on `quarter` ignores NULL) that must not mask the
        // submitted return.
        db.conn
            .execute(
                "INSERT INTO form_drafts
                    (tin, form_code, taxable_year, quarter, period_key, status, data_json)
                 VALUES (?1, '2550Q', 2026, 1, ?2, 'Draft', '{}'),
                        (?1, '2550Q', 2026, NULL, ?3, 'Draft', '{}')",
                params![tin, FilingPeriod::Quarterly(1).to_period_key(), key],
            )
            .unwrap();

        let progress = db.get_form_filing_progress(tin, "2550Q", 2026).unwrap();
        assert_eq!(progress.quarters[0], QuarterState::Draft);
        assert_eq!(progress.quarters[1], QuarterState::NotStarted);
        assert_eq!(progress.quarters[2], QuarterState::Sent);
        assert_eq!(progress.quarters[3], QuarterState::NotStarted);

        let other_year = db.get_form_filing_progress(tin, "2550Q", 2025).unwrap();
        assert_eq!(other_year.quarters[2], QuarterState::NotStarted);
    }

    #[test]
    fn monthly_form_keyed_by_item_one_date_shows_on_its_month() {
        let db = test_db();
        for (mmdd, status) in [
            (315, FilingStatus::Draft),
            (320, FilingStatus::Draft),
            (1105, FilingStatus::Draft),
        ] {
            db.save_form_draft_v2(
                "123456789000",
                "2200AN",
                2026,
                &FilingPeriod::OpenEnded(mmdd),
                &status,
                &TestDraft { value: 1 },
            )
            .unwrap();
        }
        let progress = db
            .get_form_filing_progress("123456789000", "2200AN", 2026)
            .unwrap();
        assert_eq!(progress.months[2], QuarterState::Draft);
        assert_eq!(progress.months[10], QuarterState::Draft);
        assert_eq!(progress.months[0], QuarterState::NotStarted);
        assert_eq!(progress.open_ended_count, 0);
        assert!(state_rank(&QuarterState::Queued) > state_rank(&QuarterState::Draft));
    }

    #[test]
    fn legacy_open_ended_progress_rejects_invalid_periods() {
        let db = test_db();
        let tin = "12345678900000";
        for period in [
            "002025",
            "132025",
            "2025Q0",
            "2025Q5",
            "122025Qx",
            "12322025",
            "02312025",
            "02292025",
            "03152025Q1",
            "2025Q1Q2",
            "122024",
        ] {
            assert_eq!(legacy_return_period("0605", period, 2025), None, "{period}");
            db.conn
                .execute(
                    "INSERT INTO legacy_filed_returns
                    (sha256, tin, form_identity, form_id, period, encrypted,
                     source_filename, raw_file, fields_json)
                 VALUES (?1, ?2, '0605', '0605', ?1, 1, ?1, X'00', '{}')",
                    params![period, tin],
                )
                .unwrap();
        }
        assert_eq!(
            db.get_form_filing_progress(tin, "0605", 2025)
                .unwrap()
                .open_ended_count,
            0
        );
        for (index, period) in ["122025", "2025", "2025Q1", "122025Q2", "03152025", "122025"]
            .iter()
            .enumerate()
        {
            assert!(matches!(
                legacy_return_period("0605", period, 2025),
                Some(FilingPeriod::OpenEnded(_))
            ));
            db.conn
                .execute(
                    "INSERT INTO legacy_filed_returns
                    (sha256, tin, form_identity, form_id, period, encrypted,
                     source_filename, raw_file, fields_json)
                 VALUES (?1, ?2, '0605', '0605', ?3, 1, ?1, X'00', '{}')",
                    params![format!("valid-{index}"), tin, period],
                )
                .unwrap();
        }
        assert_eq!(
            db.get_form_filing_progress(tin, "0605", 2025)
                .unwrap()
                .open_ended_count,
            5
        );
        assert!(legacy_return_period("0605", "02292024", 2024).is_some());
    }

    #[test]
    fn legacy_filed_returns_count_as_filed_in_filing_progress() {
        let db = test_db();
        // Migration stores the 14-digit `<TIN9><branch5>` tin; a profile's
        // 12-digit `<TIN9><branch3>` tin must still find its returns.
        let tin = "12345678900000";
        for (index, (form_id, period, filename)) in [
            ("1601Cv2018", "062025", "a.xml"),
            ("2551Qv2018", "2026Q1", "b.xml"),
            ("2551Qv2018", "122026Q4", "c.xml"),
            ("1701v2018", "122025", "d.xml"),
            ("1601Cv2018", "012024", "e.xml"),
            ("0605", "122025", "f.xml"),
            // A same-period copy of the 0605 (e.g. email copy) dedupes.
            ("0605", "122025", "f#copy.xml"),
            ("2200ANv2018", "03152025", "g.xml"),
        ]
        .iter()
        .enumerate()
        {
            db.conn
                .execute(
                    "INSERT INTO legacy_filed_returns
                        (sha256, tin, form_identity, form_id, period, encrypted,
                         source_filename, raw_file, fields_json)
                     VALUES (?1, ?2, ?3, ?3, ?4, 1, ?5, X'00', '{}')",
                    params![format!("sha-{index}"), tin, form_id, period, filename],
                )
                .unwrap();
        }

        // Monthly 1601C June 2025 is filed; the 2024 record does not leak in.
        let monthly = db.get_form_filing_progress(tin, "1601C", 2025).unwrap();
        assert_eq!(monthly.months[5], QuarterState::Sent);
        assert_eq!(monthly.months[0], QuarterState::NotStarted);

        // A profile's 3-digit-branch tin resolves to the same stored rows.
        let monthly = db
            .get_form_filing_progress("123456789000", "1601C", 2025)
            .unwrap();
        assert_eq!(monthly.months[5], QuarterState::Sent);

        // Quarterly 2551Q: `2026Q1` and fiscal-year `122026Q4` both file.
        let quarterly = db.get_form_filing_progress(tin, "2551Q", 2026).unwrap();
        assert_eq!(quarterly.quarters[0], QuarterState::Sent);
        assert_eq!(quarterly.quarters[3], QuarterState::Sent);
        assert_eq!(quarterly.quarters[1], QuarterState::NotStarted);

        let annual = db.get_form_filing_progress(tin, "1701", 2025).unwrap();
        assert_eq!(annual.annual_status, QuarterState::Sent);

        // Open-ended 0605: the two same-period rows count once.
        let open_ended = db.get_form_filing_progress(tin, "0605", 2025).unwrap();
        assert_eq!(open_ended.open_ended_count, 1);

        // A dated monthly return (2200AN `MMDDYYYY`) files its month.
        let dated = db.get_form_filing_progress(tin, "2200AN", 2025).unwrap();
        assert_eq!(dated.months[2], QuarterState::Sent);

        // A local draft does not downgrade the migrated filed state.
        db.conn
            .execute(
                "INSERT INTO form_drafts
                    (tin, form_code, taxable_year, period_key, status, data_json)
                 VALUES (?1, '1601C', 2025, 'M07', 'Draft', '{}')",
                params![tin],
            )
            .unwrap();
        let monthly = db.get_form_filing_progress(tin, "1601C", 2025).unwrap();
        assert_eq!(monthly.months[5], QuarterState::Sent);
        assert_eq!(monthly.months[6], QuarterState::Draft);
    }

    #[test]
    fn monthly_and_quarterly_summaries_follow_period_keys() {
        let db = test_db();
        let profile = test_profile();
        let mut monthly = editable_1601c_draft(&profile);
        monthly.month = 12;
        monthly.compute();
        db.save_1601c_draft(&monthly).unwrap();
        let quarterly = Form2551QDraft::new_from_profile(&profile, 2026, 4);
        db.save_2551q_draft(&quarterly).unwrap();

        let mut summaries = db.list_draft_summaries("123456789000", 2026).unwrap();
        summaries.sort_by(|a, b| a.form_code.cmp(&b.form_code));

        assert_eq!(summaries[0].form_code, "1601C");
        assert_eq!(summaries[0].month, Some(12));
        assert_eq!(summaries[0].quarter, None);
        assert_eq!(summaries[1].form_code, "2551Q");
        assert_eq!(summaries[1].quarter, Some(4));
        assert_eq!(summaries[1].month, None);
    }
}
