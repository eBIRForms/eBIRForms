use crate::db::{Claim1601CSubmissionResult, Claim2551QSubmissionResult, Database};
use crate::forms::form_1601c::Form1601CDraft;
use crate::forms::form_2551q::Form2551QDraft;
use crate::forms::queueable::{QueueableForm, QueueableKind};
use crate::forms::{FilingStatus, FormDraftSummary};
use crate::profile::TaxpayerProfile;
use chrono::Utc;
use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use tokio::sync::mpsc;
use tracing::{error, info, warn};

static WAKE_TX: OnceLock<mpsc::Sender<()>> = OnceLock::new();
static ACTIVE_JOBS: OnceLock<Arc<Mutex<HashSet<String>>>> = OnceLock::new();
/// Set by [`request_shutdown`]. The cron loop finishes the tick it is already
/// on, then returns, so the submission worker can drop the database.
static SHUTDOWN: AtomicBool = AtomicBool::new(false);
/// The app holds this while submission must not run (e.g. before the user
/// has signed in to the submission service). Default false.
static SUBMISSION_PAUSED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WakeDispatchStatus {
    Delivered,
    AlreadyPending,
    WorkerNotRunning,
    WorkerClosed,
}

pub fn get_active_jobs() -> Arc<Mutex<HashSet<String>>> {
    ACTIVE_JOBS
        .get_or_init(|| Arc::new(Mutex::new(HashSet::new())))
        .clone()
}

/// Stop [`start_cron_jobs`] after the tick that is already running.
///
/// Wakes a loop that is waiting out its 60 second gap so quit does not sit
/// there. A second call is a no-op aside from another wake.
pub fn request_shutdown() {
    SHUTDOWN.store(true, Ordering::SeqCst);
    wake();
}

/// Whether [`request_shutdown`] has been asked for in this process.
pub fn shutdown_requested() -> bool {
    SHUTDOWN.load(Ordering::SeqCst)
}

/// Skip queue ticks while the desktop account gate is locked.
///
/// Unlike [`request_shutdown`], this can be cleared. A later sign-in in the
/// same process files again.
pub fn set_submission_paused(paused: bool) {
    SUBMISSION_PAUSED.store(paused, Ordering::SeqCst);
    if !paused {
        // Quiet when the worker has not started yet. Launch unpauses before
        // the thread exists; the first tick runs as soon as it does.
        let _ = wake_with_status();
    }
}

pub fn submission_paused() -> bool {
    SUBMISSION_PAUSED.load(Ordering::SeqCst)
}

pub fn wake() {
    match wake_with_status() {
        WakeDispatchStatus::Delivered | WakeDispatchStatus::AlreadyPending => {}
        WakeDispatchStatus::WorkerNotRunning => {
            warn!("Background cron wake was deferred because the worker is not running yet");
        }
        WakeDispatchStatus::WorkerClosed => {
            warn!("Background cron wake was deferred because the worker channel is closed");
        }
    }
}

pub(crate) fn wake_with_status() -> WakeDispatchStatus {
    let Some(tx) = WAKE_TX.get() else {
        return WakeDispatchStatus::WorkerNotRunning;
    };

    match tx.try_send(()) {
        Ok(()) => WakeDispatchStatus::Delivered,
        Err(mpsc::error::TrySendError::Full(())) => WakeDispatchStatus::AlreadyPending,
        Err(mpsc::error::TrySendError::Closed(())) => WakeDispatchStatus::WorkerClosed,
    }
}

pub async fn start_cron_jobs<T: SubmissionTransport + 'static>(
    db: Arc<Mutex<Database>>,
    transport: Arc<T>,
) {
    info!("Background cron engine started");

    let (tx, mut rx) = mpsc::channel(1);
    let _ = WAKE_TX.set(tx);

    loop {
        if shutdown_requested() {
            break;
        }
        if submission_paused() {
            tokio::select! {
                _ = tokio::time::sleep(std::time::Duration::from_secs(60)) => {}
                _ = rx.recv() => {}
            }
            continue;
        }
        // We run a tick, then wait for either 60 seconds or a wake signal.
        run_queue_tick(db.clone(), &transport).await;
        if shutdown_requested() {
            break;
        }

        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_secs(60)) => {}
            _ = rx.recv() => {
                info!("Queue explicitly woken up by a wake signal.");
            }
        }
    }
    info!("Background cron engine stopped");
}

/// Runs a single iteration of the background job queue.
/// Extracted from `start_cron_jobs` to allow deterministic unit testing
/// without being blocked by an infinite loop or thread sleep.
pub async fn run_queue_tick<T: SubmissionTransport + 'static>(
    db: Arc<Mutex<Database>>,
    transport: &Arc<T>,
) {
    // Heartbeat log removed to prevent log spam. Job specific logs are emitted when jobs run.

    // Ensure we don't hold the DB lock across async transport calls.
    // First, fetch profiles that have background cron enabled.
    let (profiles, global_cron_enabled) = {
        let db_guard = match db.lock() {
            Ok(guard) => guard,
            Err(e) => {
                error!("Failed to lock database for cron: {}", e);
                return;
            }
        };
        let p = match db_guard.list_profiles() {
            Ok(p) => p,
            Err(e) => {
                error!("Failed to fetch profiles in cron: {}", e);
                return;
            }
        };
        let c = db_guard
            .get_setting("background_cron_enabled")
            .unwrap_or(Some("true".to_string()))
            .map(|s| s == "true")
            .unwrap_or(true);
        (p, c)
    };

    let test_enabled = profiles.iter().any(|p| p.test_notification_enabled);
    if test_enabled {
        const TITLE: &str = "BIR Vault Daemon";
        const BODY: &str = "Hello! The background cron is active.";
        if let Ok(db_guard) = db.lock() {
            let _ = db_guard.record_alert(
                None,
                crate::db::alert_kinds::CRON_HEARTBEAT,
                crate::db::AlertSeverity::Info,
                TITLE,
                BODY,
                crate::db::AlertAction::None,
            );
        }
    }

    if global_cron_enabled {
        for profile in profiles {
            // Task A: Form Submission Retries
            process_submission_queue(&profile, db.clone(), transport.clone()).await;
        }
    }
}

/// Identity of one user-reviewed queue generation. The form fingerprint binds
/// submission fields; retry timestamp and attempt count distinguish a later
/// cancel/requeue of otherwise identical content.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Queued2551QRevision {
    fingerprint: Option<String>,
    next_retry_at: Option<String>,
    submission_attempts: u32,
}

enum Queued2551QPreparation {
    Ready {
        draft: Form2551QDraft,
        revision: Queued2551QRevision,
    },
    Rejected {
        draft: Form2551QDraft,
        revision: Queued2551QRevision,
        errors: Vec<(String, String)>,
    },
    /// The row was canceled, replaced, or requeued after this job was spawned.
    Superseded,
}

fn queued_2551q_revision(draft: &Form2551QDraft) -> Option<Queued2551QRevision> {
    (matches!(draft.status, FilingStatus::Queued)
        && draft.submission_claim_token.is_none()
        && draft.submission_claimed_at.is_none())
    .then(|| Queued2551QRevision {
        fingerprint: draft.queued_submission_fingerprint.clone(),
        next_retry_at: draft.next_retry_at.clone(),
        submission_attempts: draft.submission_attempts,
    })
}

/// Pure submission-boundary preparation used both after a job is spawned and
/// immediately before network I/O. The caller loads the draft and profile under
/// one database lock, so this helper never validates against the tick's stale
/// bulk profile snapshot.
fn prepare_queued_2551q(
    mut draft: Form2551QDraft,
    profile: &TaxpayerProfile,
    expected_revision: Option<&Queued2551QRevision>,
) -> Queued2551QPreparation {
    let Some(revision) = queued_2551q_revision(&draft) else {
        return Queued2551QPreparation::Superseded;
    };
    if expected_revision.is_some_and(|expected| expected != &revision) {
        return Queued2551QPreparation::Superseded;
    }

    if let Err(error) = draft.reconcile_with_effective_profile(profile) {
        draft.revert_to_draft();
        draft.last_error = Some(format!(
            "Submission blocked because the effective taxpayer profile is unresolved: {error}"
        ));
        return Queued2551QPreparation::Rejected {
            draft,
            revision,
            errors: vec![("profile_resolution".to_string(), error)],
        };
    }
    match draft.revalidate_queued_before_submission() {
        Ok(()) => Queued2551QPreparation::Ready { draft, revision },
        Err(errors) => Queued2551QPreparation::Rejected {
            draft,
            revision,
            errors,
        },
    }
}

/// Identity of one user-reviewed 1601C queue generation. Retry metadata makes
/// cancellation followed by an identical requeue distinguishable.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Queued1601CRevision {
    fingerprint: Option<String>,
    next_retry_at: Option<String>,
    submission_attempts: u32,
}

enum Queued1601CPreparation {
    Ready {
        draft: Form1601CDraft,
        revision: Queued1601CRevision,
    },
    Rejected {
        draft: Form1601CDraft,
        revision: Queued1601CRevision,
        errors: Vec<(String, String)>,
    },
    Superseded,
}

fn queued_1601c_revision(draft: &Form1601CDraft) -> Option<Queued1601CRevision> {
    (matches!(draft.status, FilingStatus::Queued)
        && draft.submission_claim_token.is_none()
        && draft.submission_claimed_at.is_none())
    .then(|| Queued1601CRevision {
        fingerprint: draft.queued_submission_fingerprint.clone(),
        next_retry_at: draft.next_retry_at.clone(),
        submission_attempts: draft.submission_attempts,
    })
}

fn prepare_queued_1601c(
    mut draft: Form1601CDraft,
    expected_revision: Option<&Queued1601CRevision>,
) -> Queued1601CPreparation {
    let Some(revision) = queued_1601c_revision(&draft) else {
        return Queued1601CPreparation::Superseded;
    };
    if expected_revision.is_some_and(|expected| expected != &revision) {
        return Queued1601CPreparation::Superseded;
    }

    match draft.revalidate_queued_before_submission() {
        Ok(()) => Queued1601CPreparation::Ready { draft, revision },
        Err(errors) => Queued1601CPreparation::Rejected {
            draft,
            revision,
            errors,
        },
    }
}

fn validation_reason(errors: &[(String, String)]) -> String {
    errors
        .iter()
        .map(|(field, message)| format!("{field}: {message}"))
        .collect::<Vec<_>>()
        .join("; ")
}

fn queued_retry_is_due(
    next_retry_at: Option<&str>,
    now: chrono::DateTime<Utc>,
) -> Result<bool, chrono::ParseError> {
    let Some(next_retry_at) = next_retry_at else {
        return Ok(true);
    };
    let next_retry = chrono::DateTime::parse_from_rfc3339(next_retry_at)?;
    Ok(now >= next_retry.with_timezone(&Utc))
}

/// Why a transport call failed. The queue only needs the message: before
/// PUT a failure is retried, after PUT the outcome is unknown.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct TransportError(pub String);

/// The seam the queue submits through. `bir-core` ships no implementation
/// that opens a connection: the app injects one, and the tests use a
/// recording mock. `payload` is the plaintext IAF XML; any encryption and
/// the upload to BIR happen on the far side of the transport.
pub trait SubmissionTransport: Send + Sync {
    type Session: Send;

    /// A transport that simulates the PUT. Only changes the notice text.
    fn is_dry_run(&self) -> bool {
        false
    }

    fn open_session<'a>(
        &'a self,
        form_type: &'a str,
        tin: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Self::Session, TransportError>> + Send + 'a>>;

    fn store_session<'a>(
        &'a self,
        session: Self::Session,
        filename: &'a str,
        payload: &'a [u8],
    ) -> Pin<Box<dyn Future<Output = Result<(), TransportError>> + Send + 'a>>;
}

async fn process_queued_1601c_with_transport<T: SubmissionTransport>(
    summary: &FormDraftSummary,
    profile: &TaxpayerProfile,
    db: Arc<Mutex<Database>>,
    transport: &T,
) {
    let Some(month) = summary.month else {
        warn!("Cron: Refusing 1601C queue row without a monthly period");
        return;
    };
    let loaded_draft = {
        let db_guard = match db.lock() {
            Ok(guard) => guard,
            Err(_) => return,
        };
        match db_guard.get_1601c_draft(&summary.tin, summary.taxable_year, month) {
            Ok(Some(draft)) => draft,
            _ => return,
        }
    };

    let loaded_draft = if loaded_draft.submission_claim_token.is_some()
        || loaded_draft.submission_claimed_at.is_some()
    {
        let recovered = {
            let db_guard = match db.lock() {
                Ok(guard) => guard,
                Err(_) => return,
            };
            db_guard
                .recover_expired_unstarted_1601c_claim(
                    &loaded_draft.tin,
                    loaded_draft.taxable_year,
                    loaded_draft.month,
                    Utc::now(),
                )
                .unwrap_or(false)
        };
        if !recovered {
            warn!(
                "Cron: 1601C {} has an unresolved submission outcome; automatic retry is disabled and support-assisted manual reconciliation is required",
                loaded_draft.period_code()
            );
            return;
        }
        let recovered_draft = {
            let db_guard = match db.lock() {
                Ok(guard) => guard,
                Err(_) => return,
            };
            match db_guard.get_1601c_draft(&summary.tin, summary.taxable_year, month) {
                Ok(Some(draft)) => draft,
                _ => return,
            }
        };
        if recovered_draft.submission_claim_token.is_some()
            || recovered_draft.submission_claimed_at.is_some()
        {
            return;
        }
        recovered_draft
    } else {
        loaded_draft
    };

    let (mut draft, queue_revision) = match prepare_queued_1601c(loaded_draft, None) {
        Queued1601CPreparation::Ready { draft, revision } => (draft, revision),
        Queued1601CPreparation::Rejected {
            draft,
            revision,
            errors,
        } => {
            let reason = validation_reason(&errors);
            warn!(
                "Cron: Refusing invalid queued 1601C {} before XML generation: {}",
                draft.period_code(),
                reason
            );
            if let Ok(db_guard) = db.lock() {
                let _ = db_guard.replace_unclaimed_queued_1601c_submission(
                    &draft,
                    &revision.fingerprint,
                    &revision.next_retry_at,
                    revision.submission_attempts,
                );
            }
            return;
        }
        Queued1601CPreparation::Superseded => return,
    };

    match queued_retry_is_due(draft.next_retry_at.as_deref(), Utc::now()) {
        Ok(false) => return,
        Ok(true) => {}
        Err(error) => {
            warn!(
                "Cron: Refusing queued 1601C {} because its retry timestamp is invalid: {}",
                draft.period_code(),
                error
            );
            draft.revert_to_draft();
            draft.submission_error = Some(
                "Submission blocked because queued retry metadata is invalid; review and queue the return again"
                    .to_string(),
            );
            if let Ok(db_guard) = db.lock() {
                let _ = db_guard.replace_unclaimed_queued_1601c_submission(
                    &draft,
                    &queue_revision.fingerprint,
                    &queue_revision.next_retry_at,
                    queue_revision.submission_attempts,
                );
            }
            return;
        }
    }

    info!(
        "Cron: Attempting to submit queued 1601C {} for {}",
        draft.period_code(),
        draft.tin
    );
    let filename = draft.default_submission_filename();
    let xml_payload = match draft.try_to_bir_xml_payload() {
        Ok(payload) => payload,
        Err(errors) => {
            let reason = validation_reason(&errors);
            draft.revert_to_draft();
            draft.submission_error = Some(format!(
                "Submission blocked at XML generation boundary: {reason}"
            ));
            if let Ok(db_guard) = db.lock() {
                let _ = db_guard.replace_unclaimed_queued_1601c_submission(
                    &draft,
                    &queue_revision.fingerprint,
                    &queue_revision.next_retry_at,
                    queue_revision.submission_attempts,
                );
            }
            return;
        }
    };
    let Some(form_type) = crate::forms::queue_authorized_form_type_id("1601C") else {
        warn!(
            "Cron: Refusing to submit {} because 1601C is not authorized for queue submission",
            draft.period_code()
        );
        return;
    };

    let session = match transport.open_session(form_type, &draft.tin).await {
        Ok(session) => session,
        Err(error) => {
            fail_draft_1601c(
                &mut draft,
                &queue_revision,
                db.clone(),
                format!("Submission session failed before upload (no PUT attempted): {error}"),
            );
            return;
        }
    };

    // Claim only after the session is open, immediately before PUT.
    // Connect/login timeouts stay unclaimed and use the existing retry CAS.
    let claim_result = {
        let db_guard = match db.lock() {
            Ok(guard) => guard,
            Err(_) => return,
        };
        db_guard.claim_queued_1601c_submission(
            &draft.tin,
            draft.taxable_year,
            draft.month,
            &queue_revision.fingerprint,
            &queue_revision.next_retry_at,
            queue_revision.submission_attempts,
        )
    };
    let claim_token = match claim_result {
        Ok(Claim1601CSubmissionResult::Claimed {
            draft: claimed_draft,
            token,
        }) => {
            draft = claimed_draft;
            token
        }
        Ok(Claim1601CSubmissionResult::Rejected { draft, errors }) => {
            warn!(
                "Cron: 1601C submission claim rejected {}: {}",
                draft.period_code(),
                validation_reason(&errors)
            );
            return;
        }
        Ok(Claim1601CSubmissionResult::Superseded) => {
            info!(
                "Cron: 1601C submission job for {} was canceled or superseded before PUT",
                draft.period_code()
            );
            return;
        }
        Err(error) => {
            warn!(
                "Cron: Could not claim queued 1601C {} because the database claim failed: {}",
                draft.period_code(),
                error
            );
            return;
        }
    };

    let dry_run = transport.is_dry_run();
    let put_marked = {
        let db_guard = match db.lock() {
            Ok(guard) => guard,
            Err(_) => return,
        };
        db_guard
            .mark_claimed_1601c_put_started(
                &draft.tin,
                draft.taxable_year,
                draft.month,
                &claim_token,
            )
            .unwrap_or(false)
    };
    if !put_marked {
        warn!(
            "Cron: 1601C {} claim disappeared before PUT start was recorded; skipping PUT",
            draft.period_code()
        );
        return;
    }
    match transport
        .store_session(session, &filename, xml_payload.as_bytes())
        .await
    {
        Ok(()) => {
            if dry_run {
                info!(
                    "Cron: dry run: simulated submission of queued 1601C {} (nothing sent to BIR)",
                    filename
                );
            } else {
                info!("Cron: Successfully submitted queued 1601C {}", filename);
            }
            let period_label = monthly_period_label(draft.taxable_year, draft.month);
            let (notice_title, notice_body) = submission_notice(
                profile,
                "1601C",
                &period_label,
                &chrono::Local::now(),
                dry_run,
            );
            let submitted_file = filename.clone();
            // Keep the exact bytes that were PUT, so an amendment can later
            // snapshot what BIR received.
            draft.submitted_payload = Some(xml_payload);
            // `Submitted` is the queue's terminal "Sent" state: the PUT
            // succeeded. Nothing moves a return to `Confirmed` automatically.
            draft.transition_to_submitted(filename);
            if let Ok(db_guard) = db.lock() {
                match db_guard.finish_claimed_1601c_submission(&draft, &claim_token) {
                    Ok(_) => {
                        record_submission_alert(
                            &db_guard,
                            profile,
                            "1601C",
                            &period_label,
                            &submitted_file,
                            &notice_title,
                            &notice_body,
                        );
                    }
                    Err(error) => warn!(
                        "Cron: 1601C {} was transmitted but its submission claim could not be finalized: {}",
                        draft.period_code(),
                        error
                    ),
                }
            }
        }
        Err(error) => {
            warn!(
                %error,
                "Cron: 1601C PUT ended after the network claim; outcome is unknown and support-assisted manual reconciliation is required"
            );
        }
    }
}

// ===== Generic queued submission for every `QueueableForm` =====

/// The queue generation a worker loaded; every CAS names it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct QueueGeneration {
    fingerprint: Option<String>,
    next_retry_at: Option<String>,
    submission_attempts: u32,
}

fn queue_generation<F: QueueableForm>(draft: &F) -> Option<QueueGeneration> {
    let lifecycle = draft.lifecycle();
    (matches!(lifecycle.status, FilingStatus::Queued) && lifecycle.is_unclaimed()).then(|| {
        QueueGeneration {
            fingerprint: lifecycle.queued_submission_fingerprint.clone(),
            next_retry_at: lifecycle.next_retry_at.clone(),
            submission_attempts: lifecycle.submission_attempts,
        }
    })
}

/// Persist an unclaimed generation update (retry state or a return to Draft).
fn replace_queueable<F: QueueableForm>(
    db: &Arc<Mutex<Database>>,
    draft: &F,
    generation: &QueueGeneration,
) {
    if let Ok(db_guard) = db.lock() {
        match db_guard.replace_unclaimed_queued_queueable(
            draft,
            &generation.fingerprint,
            &generation.next_retry_at,
            generation.submission_attempts,
        ) {
            Ok(true) => {}
            Ok(false) => info!(
                "Cron: {} {} was canceled or superseded before its state persisted",
                F::FORM_CODE,
                draft.period_code()
            ),
            Err(error) => warn!(
                "Cron: Failed to persist {} {} queue state: {}",
                F::FORM_CODE,
                draft.period_code(),
                error
            ),
        }
    }
}

fn fail_queueable<F: QueueableForm>(
    draft: &mut F,
    generation: &QueueGeneration,
    db: &Arc<Mutex<Database>>,
    error_message: String,
) {
    warn!(
        "Cron: {} submission preparation failed for {}: {}",
        F::FORM_CODE,
        draft.period_code(),
        error_message
    );
    draft
        .lifecycle_mut()
        .record_submission_failure(error_message);
    replace_queueable(db, draft, generation);
}

/// Human-readable period for notices: `09/26`, `Q3/26` or `2026`.
fn queueable_period_label<F: QueueableForm>(draft: &F) -> String {
    match draft.filing_period() {
        crate::forms::FilingPeriod::Monthly(month) => {
            monthly_period_label(draft.taxable_year(), month)
        }
        crate::forms::FilingPeriod::Quarterly(quarter) => {
            quarterly_period_label(draft.taxable_year(), quarter)
        }
        _ => draft.taxable_year().to_string(),
    }
}

/// `authorized_form_type` is [`crate::forms::queue_authorized_form_type_id`]
/// in production: no PUT happens unless the capability registry allows it.
async fn process_queued_queueable<F: QueueableForm, T: SubmissionTransport>(
    summary: &FormDraftSummary,
    profile: &TaxpayerProfile,
    db: Arc<Mutex<Database>>,
    transport: &T,
    authorized_form_type: fn(&str) -> Option<&'static str>,
) {
    let period = i64::from(summary.month.or(summary.quarter).unwrap_or(0));
    let load = |db: &Arc<Mutex<Database>>| -> Option<F> {
        let db_guard = db.lock().ok()?;
        db_guard
            .get_queueable_draft::<F>(&summary.tin, summary.taxable_year, period)
            .ok()
            .flatten()
    };
    let Some(loaded) = load(&db) else {
        return;
    };

    // A held claim is only recoverable when its lease expired before PUT.
    let loaded = if !loaded.lifecycle().is_unclaimed() {
        let recovered = db
            .lock()
            .ok()
            .and_then(|db_guard| {
                db_guard
                    .recover_expired_unstarted_queueable_claim::<F>(
                        &summary.tin,
                        summary.taxable_year,
                        period,
                        Utc::now(),
                    )
                    .ok()
            })
            .unwrap_or(false);
        if !recovered {
            warn!(
                "Cron: {} {} has an unresolved submission outcome; automatic retry is disabled and support-assisted manual reconciliation is required",
                F::FORM_CODE,
                loaded.period_code()
            );
            return;
        }
        match load(&db) {
            Some(draft) if draft.lifecycle().is_unclaimed() => draft,
            _ => return,
        }
    } else {
        loaded
    };

    let Some(generation) = queue_generation(&loaded) else {
        return;
    };
    let mut draft = loaded;
    if let Err(errors) = draft.revalidate_queued_before_submission() {
        warn!(
            "Cron: Refusing invalid queued {} {} before XML generation: {}",
            F::FORM_CODE,
            draft.period_code(),
            validation_reason(&errors)
        );
        replace_queueable(&db, &draft, &generation);
        return;
    }

    match queued_retry_is_due(draft.lifecycle().next_retry_at.as_deref(), Utc::now()) {
        Ok(false) => return,
        Ok(true) => {}
        Err(error) => {
            warn!(
                "Cron: Refusing queued {} {} because its retry timestamp is invalid: {}",
                F::FORM_CODE,
                draft.period_code(),
                error
            );
            draft.lifecycle_mut().revert_to_draft();
            draft.lifecycle_mut().submission_error = Some(
                "Submission blocked because queued retry metadata is invalid; review and queue the return again"
                    .to_string(),
            );
            replace_queueable(&db, &draft, &generation);
            return;
        }
    }

    info!(
        "Cron: Attempting to submit queued {} {} for {}",
        F::FORM_CODE,
        draft.period_code(),
        draft.tin()
    );
    let filename = draft.submission_filename();
    let payload = match draft.official_payload() {
        Ok(payload) => payload,
        Err(errors) => {
            let reason = validation_reason(&errors);
            draft.lifecycle_mut().revert_to_draft();
            draft.lifecycle_mut().submission_error = Some(format!(
                "Submission blocked at XML generation boundary: {reason}"
            ));
            replace_queueable(&db, &draft, &generation);
            return;
        }
    };
    let Some(form_type) = authorized_form_type(F::FORM_CODE) else {
        warn!(
            "Cron: Refusing to submit {} because {} is not authorized for queue submission",
            draft.period_code(),
            F::FORM_CODE
        );
        return;
    };

    let session = match transport.open_session(form_type, draft.tin()).await {
        Ok(session) => session,
        Err(error) => {
            fail_queueable(
                &mut draft,
                &generation,
                &db,
                format!("Submission session failed before upload (no PUT attempted): {error}"),
            );
            return;
        }
    };

    // Claim only after the session is open, immediately before PUT.
    let claim = {
        let Ok(db_guard) = db.lock() else {
            return;
        };
        db_guard.claim_queued_queueable::<F>(
            draft.tin(),
            draft.taxable_year(),
            draft.period_column(),
            &generation.fingerprint,
            &generation.next_retry_at,
            generation.submission_attempts,
        )
    };
    let claim_token = match claim {
        Ok(crate::db::ClaimQueueableResult::Claimed {
            draft: claimed,
            token,
        }) => {
            draft = claimed;
            token
        }
        Ok(crate::db::ClaimQueueableResult::Rejected { draft, errors }) => {
            warn!(
                "Cron: {} submission claim rejected {}: {}",
                F::FORM_CODE,
                draft.period_code(),
                validation_reason(&errors)
            );
            return;
        }
        Ok(crate::db::ClaimQueueableResult::Superseded) => {
            info!(
                "Cron: {} submission job for {} was canceled or superseded before PUT",
                F::FORM_CODE,
                draft.period_code()
            );
            return;
        }
        Err(error) => {
            warn!(
                "Cron: Could not claim queued {} {} because the database claim failed: {}",
                F::FORM_CODE,
                draft.period_code(),
                error
            );
            return;
        }
    };

    let dry_run = transport.is_dry_run();
    let put_marked = db
        .lock()
        .ok()
        .and_then(|db_guard| {
            db_guard
                .mark_claimed_queueable_put_started::<F>(
                    draft.tin(),
                    draft.taxable_year(),
                    draft.period_column(),
                    &claim_token,
                )
                .ok()
        })
        .unwrap_or(false);
    if !put_marked {
        warn!(
            "Cron: {} {} claim disappeared before PUT start was recorded; skipping PUT",
            F::FORM_CODE,
            draft.period_code()
        );
        return;
    }

    match transport
        .store_session(session, &filename, payload.as_bytes())
        .await
    {
        Ok(()) => {
            if dry_run {
                info!(
                    "Cron: dry run: simulated submission of queued {} {} (nothing sent to BIR)",
                    F::FORM_CODE,
                    filename
                );
            } else {
                info!(
                    "Cron: Successfully submitted queued {} {}",
                    F::FORM_CODE,
                    filename
                );
            }
            let period_label = queueable_period_label(&draft);
            let (notice_title, notice_body) = submission_notice(
                profile,
                F::FORM_CODE,
                &period_label,
                &chrono::Local::now(),
                dry_run,
            );
            let submitted_file = filename.clone();
            // Keep the exact bytes that were PUT, so an amendment can later
            // snapshot what BIR received.
            draft.lifecycle_mut().submitted_payload = Some(payload);
            // Terminal "Sent": see the 1601C worker.
            draft.lifecycle_mut().transition_to_submitted(filename);
            if let Ok(db_guard) = db.lock() {
                match db_guard.finish_claimed_queueable_submission(&draft, &claim_token) {
                    Ok(_) => {
                        record_submission_alert(
                            &db_guard,
                            profile,
                            F::FORM_CODE,
                            &period_label,
                            &submitted_file,
                            &notice_title,
                            &notice_body,
                        );
                    }
                    Err(error) => warn!(
                        "Cron: {} {} was transmitted but its submission claim could not be finalized: {}",
                        F::FORM_CODE,
                        draft.period_code(),
                        error
                    ),
                }
            }
        }
        Err(_) => {
            warn!(
                "Cron: {} PUT ended after the network claim; outcome is unknown and support-assisted manual reconciliation is required",
                F::FORM_CODE
            );
        }
    }
}

/// Run the generic worker for a queueable form code. `false` when the code
/// is not a [`QueueableKind`].
async fn process_queued_by_kind<T: SubmissionTransport>(
    summary: &FormDraftSummary,
    profile: &TaxpayerProfile,
    db: Arc<Mutex<Database>>,
    transport: &T,
) -> bool {
    let Some(kind) = QueueableKind::from_form_code(&summary.form_code) else {
        return false;
    };
    crate::with_queueable_kind!(kind, F => {
        process_queued_queueable::<F, _>(
            summary,
            profile,
            db,
            transport,
            crate::forms::queue_authorized_form_type_id,
        )
        .await
    });
    true
}

#[cfg(test)]
fn submission_queue_year_at<Tz: chrono::TimeZone>(now: &chrono::DateTime<Tz>) -> u16 {
    use chrono::Datelike;
    now.year() as u16
}

async fn process_submission_queue<T: SubmissionTransport + 'static>(
    profile: &TaxpayerProfile,
    db: Arc<Mutex<Database>>,
    transport: Arc<T>,
) {
    let summaries = {
        let db_guard = match db.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        db_guard
            .list_all_queued_submissions()
            .unwrap_or_default()
            .into_iter()
            .filter(|s| s.tin == profile.tin.full())
            .collect::<Vec<_>>()
    };

    for summary in summaries
        .into_iter()
        .filter(|s| s.status == FilingStatus::Queued)
        .filter(|s| crate::forms::can_queue_for_submission(&s.form_code))
    {
        let job_key = format!(
            "form:{}:{}:{}:{}",
            summary.form_code,
            profile.tin.full(),
            summary.taxable_year,
            summary.month.unwrap_or(summary.quarter.unwrap_or(0))
        );

        let active_jobs = get_active_jobs();
        {
            let mut jobs = match active_jobs.lock() {
                Ok(j) => j,
                Err(_) => continue,
            };
            if !jobs.insert(job_key.clone()) {
                continue; // Already running
            }
        }

        let profile_clone = profile.clone();
        let db_clone = db.clone();
        let active_jobs_clone = active_jobs.clone();
        let transport = transport.clone();

        tokio::spawn(async move {
            let _cleanup = JobCleanup {
                key: job_key,
                active_jobs: active_jobs_clone,
            };

            if summary.form_code == "2551Q" {
                let (loaded_draft, current_profile) = {
                    let db_guard = match db_clone.lock() {
                        Ok(g) => g,
                        Err(_) => return,
                    };
                    let current_profile = match db_guard.get_profile(&profile_clone.tin.full()) {
                        Ok(Some(profile)) => profile,
                        _ => return,
                    };
                    let draft = match db_guard.get_2551q_draft(
                        &current_profile.tin.full(),
                        summary.taxable_year,
                        summary.quarter.unwrap_or(0),
                    ) {
                        Ok(Some(draft)) => draft,
                        _ => return,
                    };
                    (draft, current_profile)
                };

                if loaded_draft.submission_claim_token.is_some()
                    || loaded_draft.submission_claimed_at.is_some()
                {
                    let recovered = {
                        let db_guard = match db_clone.lock() {
                            Ok(guard) => guard,
                            Err(_) => return,
                        };
                        db_guard
                            .recover_expired_unstarted_2551q_claim(
                                &loaded_draft.tin,
                                loaded_draft.taxable_year,
                                loaded_draft.quarter,
                                Utc::now(),
                            )
                            .unwrap_or(false)
                    };
                    if !recovered {
                        warn!(
                            "Cron: Form {} has an unresolved submission outcome; automatic retry is disabled and support-assisted manual reconciliation is required",
                            loaded_draft.period_code()
                        );
                        return;
                    }
                    return;
                }

                let (mut draft, queue_revision) =
                    match prepare_queued_2551q(loaded_draft, &current_profile, None) {
                        Queued2551QPreparation::Ready { draft, revision } => (draft, revision),
                        Queued2551QPreparation::Rejected {
                            draft,
                            revision,
                            errors,
                        } => {
                            let reason = validation_reason(&errors);
                            warn!(
                                "Cron: Refusing invalid queued form {} before XML generation: {}",
                                draft.period_code(),
                                reason
                            );
                            if let Ok(db_guard) = db_clone.lock() {
                                let _ = db_guard.replace_unclaimed_queued_2551q_submission(
                                    &draft,
                                    &revision.fingerprint,
                                    &revision.next_retry_at,
                                    revision.submission_attempts,
                                );
                            }
                            return;
                        }
                        Queued2551QPreparation::Superseded => return,
                    };

                match queued_retry_is_due(draft.next_retry_at.as_deref(), Utc::now()) {
                    Ok(false) => return,
                    Ok(true) => {}
                    Err(error) => {
                        warn!(
                            "Cron: Refusing queued form {} because its retry timestamp is invalid: {}",
                            draft.period_code(),
                            error
                        );
                        draft.revert_to_draft();
                        draft.last_error = Some(
                            "Submission blocked because queued retry metadata is invalid; review and queue the return again"
                                .to_string(),
                        );
                        if let Ok(db_guard) = db_clone.lock() {
                            let _ = db_guard.replace_unclaimed_queued_2551q_submission(
                                &draft,
                                &queue_revision.fingerprint,
                                &queue_revision.next_retry_at,
                                queue_revision.submission_attempts,
                            );
                        }
                        return;
                    }
                }

                info!(
                    "Cron: Attempting to submit queued form {} for {}",
                    draft.period_code(),
                    current_profile.tin.full()
                );

                let filename = draft.default_submission_filename();
                let xml_payload = match draft.to_bir_xml_payload() {
                    Ok(payload) => payload,
                    Err(errors) => {
                        let reason = errors
                            .iter()
                            .map(|(field, message)| format!("{field}: {message}"))
                            .collect::<Vec<_>>()
                            .join("; ");
                        warn!(
                            "Cron: Refusing invalid form {} at XML generation boundary: {}",
                            draft.period_code(),
                            reason
                        );
                        draft.revert_to_draft();
                        draft.last_error = Some(format!(
                            "Submission blocked at XML generation boundary: {reason}"
                        ));
                        if let Ok(db_guard) = db_clone.lock() {
                            let _ = db_guard.replace_unclaimed_queued_2551q_submission(
                                &draft,
                                &queue_revision.fingerprint,
                                &queue_revision.next_retry_at,
                                queue_revision.submission_attempts,
                            );
                        }
                        return;
                    }
                };
                // Resolve the exact transport identifier from the audited
                // capability registry at the irreversible boundary. Never
                // revive an uncertified submitter with a hard-coded fallback.
                let Some(form_type) = crate::forms::queue_authorized_form_type_id("2551Q") else {
                    warn!(
                        "Cron: Refusing to submit {} because 2551Q is not authorized for queue submission",
                        draft.period_code()
                    );
                    return;
                };

                let session = match transport.open_session(form_type, &draft.tin).await {
                    Ok(session) => session,
                    Err(error) => {
                        fail_draft_2551q(
                            &mut draft,
                            &queue_revision,
                            db_clone.clone(),
                            format!(
                                "Submission session failed before upload (no PUT attempted): {error}"
                            ),
                        );
                        return;
                    }
                };

                // Claim only after the session is open, immediately before PUT.
                let claim_result = {
                    let db_guard = match db_clone.lock() {
                        Ok(guard) => guard,
                        Err(_) => return,
                    };
                    db_guard.claim_queued_2551q_submission(
                        &draft.tin,
                        draft.taxable_year,
                        draft.quarter,
                        &queue_revision.fingerprint,
                        &queue_revision.next_retry_at,
                        queue_revision.submission_attempts,
                    )
                };
                let claim_token = match claim_result {
                    Ok(Claim2551QSubmissionResult::Claimed {
                        draft: claimed_draft,
                        token,
                    }) => {
                        draft = claimed_draft;
                        // Let an open form window replace its stale Queued copy
                        // with the claimed row before PUT returns.
                        token
                    }
                    Ok(Claim2551QSubmissionResult::Rejected {
                        draft: rejected_draft,
                        errors,
                    }) => {
                        let reason = validation_reason(&errors);
                        warn!(
                            "Cron: Submission claim rejected form {}: {}",
                            rejected_draft.period_code(),
                            reason
                        );
                        return;
                    }
                    Ok(Claim2551QSubmissionResult::Superseded) => {
                        info!(
                            "Cron: Submission job for {} was canceled or superseded before PUT",
                            draft.period_code()
                        );
                        return;
                    }
                    Err(_) => {
                        warn!(
                            "Cron: Could not claim queued form {} because the database claim failed",
                            draft.period_code()
                        );
                        return;
                    }
                };

                let dry_run = transport.is_dry_run();
                let put_marked = {
                    let db_guard = match db_clone.lock() {
                        Ok(guard) => guard,
                        Err(_) => return,
                    };
                    db_guard
                        .mark_claimed_2551q_put_started(
                            &draft.tin,
                            draft.taxable_year,
                            draft.quarter,
                            &claim_token,
                        )
                        .unwrap_or(false)
                };
                if !put_marked {
                    warn!(
                        "Cron: Form {} claim disappeared before PUT start was recorded; skipping PUT",
                        draft.period_code()
                    );
                    return;
                }
                match transport
                    .store_session(session, &filename, xml_payload.as_bytes())
                    .await
                {
                    Ok(()) => {
                        if dry_run {
                            info!(
                                "Cron: dry run: simulated submission of queued form {} (nothing sent to BIR)",
                                filename
                            );
                        } else {
                            info!("Cron: Successfully submitted queued form {}", filename);
                        }
                        let period_label =
                            quarterly_period_label(draft.taxable_year, draft.quarter);
                        let (notice_title, notice_body) = submission_notice(
                            &current_profile,
                            "2551Q",
                            &period_label,
                            &chrono::Local::now(),
                            dry_run,
                        );
                        // Keep the exact bytes that were PUT.
                        draft.submitted_payload = Some(xml_payload);
                        // Terminal "Sent": see the 1601C worker.
                        draft.transition_to_submitted(filename.clone());
                        if let Ok(db_guard) = db_clone.lock() {
                            match db_guard.finish_claimed_2551q_submission(&draft, &claim_token) {
                                Ok(_) => {
                                    record_submission_alert(
                                        &db_guard,
                                        &current_profile,
                                        "2551Q",
                                        &period_label,
                                        &filename,
                                        &notice_title,
                                        &notice_body,
                                    );
                                }
                                Err(error) => warn!(
                                    "Cron: Form {} was transmitted but its submission claim could not be finalized: {}",
                                    draft.period_code(),
                                    error
                                ),
                            }
                        }
                    }
                    Err(error) => {
                        // Once PUT has started, an error does not prove that BIR
                        // received no bytes. Keep the durable claim just like a
                        // process crash: retrying could duplicate a return.
                        warn!(
                            %error,
                            "Cron: Submission PUT ended after the network claim; outcome is unknown and support-assisted manual reconciliation is required"
                        );
                    }
                }
            } else if summary.form_code == "1601C" {
                process_queued_1601c_with_transport(
                    &summary,
                    &profile_clone,
                    db_clone.clone(),
                    &*transport,
                )
                .await;
            } else {
                process_queued_by_kind(&summary, &profile_clone, db_clone.clone(), &*transport)
                    .await;
            }
        });
    }
}

/// `09/26` for a monthly return — the shape the maintainer reads at a glance.
pub fn monthly_period_label(taxable_year: u16, month: u8) -> String {
    format!("{month:02}/{:02}", taxable_year % 100)
}

/// `Q3/26` for a quarterly return.
pub fn quarterly_period_label(taxable_year: u16, quarter: u8) -> String {
    format!("Q{quarter}/{:02}", taxable_year % 100)
}

/// Desktop banner for a successful PUT: who, which return, when — not the
/// IAF filename, which says none of that at a glance. Returns `(title, body)`.
/// Appended to the submission notice title and body when the transport
/// only simulated the PUT ([`SubmissionTransport::is_dry_run`]).
pub const DRY_RUN_NOTICE_SUFFIX: &str = " (dry run \u{2014} nothing was sent to BIR)";

pub fn submission_notice<Tz: chrono::TimeZone>(
    profile: &TaxpayerProfile,
    form_code: &str,
    period_label: &str,
    sent_at: &chrono::DateTime<Tz>,
    dry_run: bool,
) -> (String, String)
where
    Tz::Offset: std::fmt::Display,
{
    let mut title = format!(
        "{form_code} {period_label} submitted \u{2014} {}",
        profile.full_name
    );
    let mut body = format!(
        "TIN {}\nSent {}\nFiled under {}",
        profile.tin.formatted(),
        sent_at.format("%I:%M %p, %-d %b %Y"),
        profile.inbox_email()
    );
    if dry_run {
        title.push_str(DRY_RUN_NOTICE_SUFFIX);
        body.push('\n');
        body.push_str(DRY_RUN_NOTICE_SUFFIX.trim_start());
    }
    (title, body)
}

/// BIR's received stamp the way the confirmation email prints it. The
/// receipt table stores it normalized (`2026-09-12` / `12:11:00`); a banner
/// should read `12 September 2026, 12:11 PM`. Unrecognized shapes pass through.
pub fn display_received_at(received_date: &str, received_time: &str) -> String {
    let date = chrono::NaiveDate::parse_from_str(received_date.trim(), "%Y-%m-%d")
        .map(|d| d.format("%-d %B %Y").to_string())
        .unwrap_or_else(|_| received_date.trim().to_string());
    let time = chrono::NaiveTime::parse_from_str(received_time.trim(), "%H:%M:%S")
        .or_else(|_| chrono::NaiveTime::parse_from_str(received_time.trim(), "%H:%M"))
        .map(|t| t.format("%I:%M %p").to_string())
        .unwrap_or_else(|_| received_time.trim().to_string());
    format!("{date}, {time}")
}

/// `1601Cv2018` → `1601C`: the form code without the BIR schema revision.
pub fn form_code_from_form_type(form_type: &str) -> &str {
    form_type.split('v').next().unwrap_or(form_type)
}

/// Same event on the in-app Notifications page, so it is still there after the
/// banner is gone. One entry per return period; a re-file of the same period
/// bumps that entry rather than adding a second.
fn record_submission_alert(
    db: &Database,
    profile: &TaxpayerProfile,
    form_code: &str,
    period_label: &str,
    filename: &str,
    title: &str,
    body: &str,
) {
    let kind = format!("form_submitted:{form_code}:{period_label}");
    let detail = format!("{}\n{body}\nFile {filename}", profile.full_name);
    let _ = db.record_alert(
        Some(&profile.tin.full()),
        &kind,
        crate::db::AlertSeverity::Info,
        title,
        &detail,
        crate::db::AlertAction::None,
    );
}

fn fail_draft_2551q(
    draft: &mut crate::forms::form_2551q::Form2551QDraft,
    revision: &Queued2551QRevision,
    db: Arc<Mutex<Database>>,
    error_msg: String,
) {
    warn!(
        "Cron: Submission failed for {}: {}",
        draft.period_code(),
        error_msg
    );
    let attempts_before = draft.submission_attempts;
    draft.record_submission_failure(error_msg);

    if draft.submission_attempts >= 5 || attempts_before >= 4 {
        warn!(
            "Cron: Max attempts reached for {}. Giving up.",
            draft.period_code()
        );
    } else {
        let delay_mins = 2i64.pow(draft.submission_attempts - 1);
        info!("Cron: Next retry scheduled in {} mins", delay_mins);
    }

    if let Ok(db_guard) = db.lock() {
        match db_guard.replace_unclaimed_queued_2551q_submission(
            draft,
            &revision.fingerprint,
            &revision.next_retry_at,
            revision.submission_attempts,
        ) {
            Ok(true) => {}
            Ok(false) => info!(
                "Cron: Form {} was canceled or superseded before retry state persisted",
                draft.period_code()
            ),
            Err(error) => warn!(
                "Cron: Failed to persist submission failure for {}: {}",
                draft.period_code(),
                error
            ),
        }
    }
}

fn fail_draft_1601c(
    draft: &mut Form1601CDraft,
    revision: &Queued1601CRevision,
    db: Arc<Mutex<Database>>,
    error_message: String,
) {
    warn!(
        "Cron: 1601C submission preparation failed for {}: {}",
        draft.period_code(),
        error_message
    );
    draft.record_submission_failure(error_message);
    if let Ok(db_guard) = db.lock() {
        match db_guard.replace_unclaimed_queued_1601c_submission(
            draft,
            &revision.fingerprint,
            &revision.next_retry_at,
            revision.submission_attempts,
        ) {
            Ok(true) => {}
            Ok(false) => info!(
                "Cron: 1601C {} was canceled or superseded before retry state persisted",
                draft.period_code()
            ),
            Err(error) => warn!(
                "Cron: Failed to persist 1601C submission preparation failure for {}: {}",
                draft.period_code(),
                error
            ),
        }
    }
}

struct JobCleanup {
    key: String,
    active_jobs: Arc<Mutex<HashSet<String>>>,
}

impl Drop for JobCleanup {
    fn drop(&mut self) {
        if let Ok(mut jobs) = self.active_jobs.lock() {
            jobs.remove(&self.key);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn period_labels_read_as_month_or_quarter_over_two_digit_year() {
        assert_eq!(monthly_period_label(2026, 9), "09/26");
        assert_eq!(monthly_period_label(2026, 12), "12/26");
        assert_eq!(quarterly_period_label(2026, 3), "Q3/26");
    }

    #[test]
    fn submission_notice_marks_dry_run_in_title_and_body() {
        let profile = crate::filing_queue::mandatory_lab_1601c_profile();
        let sent_at = chrono::FixedOffset::east_opt(8 * 3600)
            .unwrap()
            .with_ymd_and_hms(2026, 9, 12, 15, 17, 25)
            .unwrap();
        let (title, body) = submission_notice(&profile, "1601C", "09/26", &sent_at, true);
        assert_eq!(
            title,
            "1601C 09/26 submitted \u{2014} Juan Dela Cruz (dry run \u{2014} nothing was sent to BIR)"
        );
        assert_eq!(
            body,
            "TIN 000-000-000-00000\nSent 03:17 PM, 12 Sep 2026\nFiled under taxpayer@example.com\n(dry run \u{2014} nothing was sent to BIR)"
        );
    }

    #[test]
    fn submission_notice_names_taxpayer_tin_form_period_and_time() {
        let profile = crate::filing_queue::mandatory_lab_1601c_profile();
        let sent_at = chrono::FixedOffset::east_opt(8 * 3600)
            .unwrap()
            .with_ymd_and_hms(2026, 9, 12, 15, 17, 25)
            .unwrap();
        let (title, body) = submission_notice(&profile, "1601C", "09/26", &sent_at, false);
        assert_eq!(title, "1601C 09/26 submitted \u{2014} Juan Dela Cruz");
        assert_eq!(
            body,
            "TIN 000-000-000-00000\nSent 03:17 PM, 12 Sep 2026\nFiled under taxpayer@example.com"
        );
    }

    #[test]
    fn received_stamp_and_form_code_read_as_bir_prints_them() {
        assert_eq!(
            display_received_at("2026-09-12", "12:11:00"),
            "12 September 2026, 12:11 PM"
        );
        assert_eq!(
            display_received_at("2026-09-12", "11:43"),
            "12 September 2026, 11:43 AM"
        );
        assert_eq!(
            display_received_at("12 September 2026", "11:43 AM"),
            "12 September 2026, 11:43 AM"
        );
        assert_eq!(form_code_from_form_type("1601Cv2018"), "1601C");
        assert_eq!(form_code_from_form_type("2551Qv2018"), "2551Q");
    }

    #[test]
    fn submission_alert_lands_on_the_notifications_page_once_per_period() {
        let db = Database::open_in_memory_for_tests().expect("db");
        let profile = crate::filing_queue::mandatory_lab_1601c_profile();
        let profile = db.save_profile(profile).expect("profile");
        for _ in 0..2 {
            record_submission_alert(
                &db,
                &profile,
                "1601C",
                "09/26",
                "00000000000000-1601Cv2018-092026#x#.xml",
                "1601C 09/26 submitted \u{2014} Juan Dela Cruz",
                "TIN 000-000-000-00000\nSent 03:17 PM, 12 Sep 2026",
            );
        }
        let alerts = db.list_active_alerts(None).expect("alerts");
        let ours: Vec<_> = alerts
            .iter()
            .filter(|alert| alert.kind == "form_submitted:1601C:09/26")
            .collect();
        assert_eq!(ours.len(), 1, "{alerts:?}");
        assert_eq!(ours[0].severity, crate::db::AlertSeverity::Info);
        assert_eq!(ours[0].action, crate::db::AlertAction::None);
        assert!(
            ours[0].detail.contains("Juan Dela Cruz"),
            "{}",
            ours[0].detail
        );
        assert!(ours[0].detail.contains("TIN 000-000-000-00000"));
        assert!(ours[0].detail.contains("092026"));
    }

    use super::*;
    use crate::db::{Database, Job};
    use crate::forms::form_1601c::Form1601CDraft;
    use crate::forms::form_2551q::Item13Election;
    use crate::profile::{EoptTier, IncomeTaxElection, TaxElectionHistory, TaxpayerProfile};
    use chrono::{Datelike, TimeZone};
    use tempfile::NamedTempFile;

    fn is_email_poll_job(job: &Job) -> bool {
        job.command
            .as_deref()
            .is_some_and(|command| command.starts_with("bir_poll_email "))
            || job.name.contains(" confirmation for ")
            || job.name.contains("confirmation email")
            || job.name.starts_with("Poll Receipts:")
    }

    #[test]
    fn retry_gate_is_due_only_for_absent_or_elapsed_valid_timestamps() {
        let now = Utc
            .with_ymd_and_hms(2026, 7, 24, 12, 0, 0)
            .single()
            .unwrap();

        assert!(queued_retry_is_due(None, now).unwrap());
        assert!(queued_retry_is_due(Some("2026-07-24T11:59:59Z"), now).unwrap());
        assert!(!queued_retry_is_due(Some("2026-07-24T12:00:01Z"), now).unwrap());
    }

    #[test]
    fn retry_gate_rejects_malformed_non_null_metadata() {
        let now = Utc
            .with_ymd_and_hms(2026, 7, 24, 12, 0, 0)
            .single()
            .unwrap();

        assert!(queued_retry_is_due(Some("not-rfc3339"), now).is_err());
        assert!(queued_retry_is_due(Some(""), now).is_err());
    }

    fn test_profile() -> TaxpayerProfile {
        serde_json::from_value(serde_json::json!({
            "id": null,
            "full_name": "Queue Guard Taxpayer",
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
            "email": "guard@example.com",
            "default_form_type": "2551Qv2018",
            "taxpayer_type": "Individual",
            "business_start_date": "2010-01-01"
        }))
        .unwrap()
    }

    fn reviewed_profile() -> TaxpayerProfile {
        let mut profile = test_profile();
        profile.tax_elections.push(TaxElectionHistory {
            taxable_year: 2024,
            election: IncomeTaxElection::GraduatedUnspecified,
            elected_at: chrono::NaiveDateTime::default(),
            source_form: "2551Qv2018".to_string(),
        });
        profile.ensure_profile_version_ledger();
        profile
    }

    fn queued_draft(profile: &TaxpayerProfile) -> Form2551QDraft {
        let mut draft = Form2551QDraft::new_from_effective_profile(profile, 2024, 1);
        draft.item_13_election = Item13Election::Graduated;
        draft
            .transition_to_queued()
            .expect("the reviewed draft should queue");
        draft
    }

    fn queued_1601c_draft(profile: &TaxpayerProfile) -> Form1601CDraft {
        let mut draft = Form1601CDraft::new_from_profile(profile, 2024, 5);
        draft.any_taxes_withheld = false;
        draft
            .transition_to_queued()
            .expect("the reviewed 1601C draft should queue");
        draft
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct RecordedSubmission {
        form_type: String,
        filename: String,
        payload: Vec<u8>,
    }

    #[derive(Debug, Clone, Copy)]
    enum TestTransportOutcome {
        Success,
        PreStore,
        UnknownIo,
    }

    struct RecordingSubmissionTransport {
        calls: Mutex<Vec<RecordedSubmission>>,
        outcome: TestTransportOutcome,
        dry_run: bool,
    }

    impl RecordingSubmissionTransport {
        fn new(outcome: TestTransportOutcome) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                outcome,
                dry_run: false,
            }
        }

        fn dry_run(outcome: TestTransportOutcome) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                outcome,
                dry_run: true,
            }
        }

        fn calls(&self) -> Vec<RecordedSubmission> {
            self.calls
                .lock()
                .expect("recorded transport calls should not be poisoned")
                .clone()
        }
    }

    impl SubmissionTransport for RecordingSubmissionTransport {
        type Session = String;

        fn is_dry_run(&self) -> bool {
            self.dry_run
        }

        fn open_session<'a>(
            &'a self,
            form_type: &'a str,
            _tin: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Self::Session, TransportError>> + Send + 'a>>
        {
            let outcome = self.outcome;
            let form_type = form_type.to_string();
            Box::pin(async move {
                match outcome {
                    TestTransportOutcome::PreStore => {
                        Err(TransportError("simulated connect timeout".to_string()))
                    }
                    TestTransportOutcome::Success | TestTransportOutcome::UnknownIo => {
                        Ok(form_type)
                    }
                }
            })
        }

        fn store_session<'a>(
            &'a self,
            session: Self::Session,
            filename: &'a str,
            payload: &'a [u8],
        ) -> Pin<Box<dyn Future<Output = Result<(), TransportError>> + Send + 'a>> {
            self.calls
                .lock()
                .expect("recorded transport calls should not be poisoned")
                .push(RecordedSubmission {
                    form_type: session,
                    filename: filename.to_string(),
                    payload: payload.to_vec(),
                });
            let outcome = self.outcome;
            Box::pin(async move {
                match outcome {
                    TestTransportOutcome::Success => Ok(()),
                    TestTransportOutcome::UnknownIo => Err(TransportError(
                        "simulated unknown network outcome".to_string(),
                    )),
                    TestTransportOutcome::PreStore => {
                        panic!("PUT must not run after a pre-store session failure")
                    }
                }
            })
        }
    }

    fn queued_1601c_summary(id: i64, draft: &Form1601CDraft) -> FormDraftSummary {
        FormDraftSummary {
            id,
            tin: draft.tin.clone(),
            form_code: "1601C".to_string(),
            taxable_year: draft.taxable_year,
            quarter: None,
            month: Some(draft.month),
            status: FilingStatus::Queued,
            updated_at: draft.updated_at.clone(),
        }
    }

    fn assert_exact_1601c_transport_call(call: &RecordedSubmission, queued: &Form1601CDraft) {
        assert_eq!(call.form_type, "1601Cv2018");
        assert_eq!(call.filename, queued.default_submission_filename());
        let expected_xml = queued
            .try_to_bir_xml_payload()
            .expect("the reviewed queued draft should produce exact XML");
        // The transport receives the plaintext XML; encryption is server-side.
        assert_eq!(call.payload, expected_xml.as_bytes());
        let sent_xml = String::from_utf8(call.payload.clone())
            .expect("the transport payload should contain UTF-8 XML");
        assert_eq!(
            crate::bir_xml::parse_bir_xml_with_codec_checked(
                &sent_xml,
                bir_rules::serialization::BodyCodec::Utf8PercentRfc3986Unreserved,
            )
            .expect("the generated transport payload should contain checked UTF-8 BIR XML"),
            queued.to_bir_field_map()
        );
    }

    #[tokio::test]
    async fn test_run_queue_tick_does_not_panic() {
        let db_file = NamedTempFile::new().unwrap();
        // Since we are just testing the runner, if keyring fails we can't test.
        let db = match Database::open(db_file.path()) {
            Ok(db) => db,
            Err(_) => return, // Skip test if keychain fails
        };
        let db = Arc::new(Mutex::new(db));

        // Just run the tick on an empty database. Should not panic.
        let transport = Arc::new(RecordingSubmissionTransport::new(
            TestTransportOutcome::Success,
        ));
        run_queue_tick(db.clone(), &transport).await;
        assert!(transport.calls().is_empty());
    }

    #[test]
    fn submission_preparation_uses_current_profile_and_rejects_changes() {
        let mut reviewed_profile = reviewed_profile();
        reviewed_profile.eopt_tier = Some(EoptTier::Medium);
        reviewed_profile.profile_versions[0].eopt_tier = Some(EoptTier::Medium);
        let draft = queued_draft(&reviewed_profile);

        let mut current_profile = reviewed_profile;
        current_profile.eopt_tier = Some(EoptTier::Micro);
        current_profile.profile_versions[0].eopt_tier = Some(EoptTier::Micro);
        let prepared = prepare_queued_2551q(draft, &current_profile, None);

        match prepared {
            Queued2551QPreparation::Rejected { draft, errors, .. } => {
                assert_eq!(draft.status, FilingStatus::Draft);
                assert!(errors.iter().any(|(field, _)| field == "profile_snapshot"));
            }
            _ => panic!("a changed current profile must reject the queued return"),
        }
    }

    #[test]
    fn submission_preparation_rejects_an_unresolved_effective_profile() {
        let reviewed_profile = reviewed_profile();
        let draft = queued_draft(&reviewed_profile);
        let mut current_profile = reviewed_profile;
        current_profile.profile_versions.clear();

        match prepare_queued_2551q(draft, &current_profile, None) {
            Queued2551QPreparation::Rejected { draft, errors, .. } => {
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
            _ => panic!("an unresolved effective profile must reject the queued return"),
        }
    }

    #[test]
    fn rejected_2551q_preparation_preserves_the_original_queue_revision() {
        let mut reviewed_profile = reviewed_profile();
        reviewed_profile.eopt_tier = Some(EoptTier::Medium);
        reviewed_profile.profile_versions[0].eopt_tier = Some(EoptTier::Medium);
        let mut draft = queued_draft(&reviewed_profile);
        draft.submission_attempts = 3;
        draft.next_retry_at = Some("2099-01-01T00:00:00Z".to_string());
        let expected_revision =
            queued_2551q_revision(&draft).expect("the unclaimed queue revision should exist");

        let mut current_profile = reviewed_profile;
        current_profile.eopt_tier = Some(EoptTier::Micro);
        current_profile.profile_versions[0].eopt_tier = Some(EoptTier::Micro);

        match prepare_queued_2551q(draft, &current_profile, None) {
            Queued2551QPreparation::Rejected {
                draft, revision, ..
            } => {
                assert_eq!(revision, expected_revision);
                assert_eq!(draft.status, FilingStatus::Draft);
                assert!(draft.queued_submission_fingerprint.is_none());
                assert!(draft.next_retry_at.is_none());
                assert_eq!(draft.submission_attempts, 0);
            }
            _ => panic!("a rejected preparation must retain its source queue revision"),
        }
    }

    #[test]
    fn submission_revision_rejects_cancel_and_identical_requeue() {
        let profile = reviewed_profile();
        let draft = queued_draft(&profile);
        let revision = match prepare_queued_2551q(draft.clone(), &profile, None) {
            Queued2551QPreparation::Ready { revision, .. } => revision,
            _ => panic!("the unchanged queued return must prepare"),
        };

        let mut canceled = draft.clone();
        canceled.revert_to_draft();
        assert!(matches!(
            prepare_queued_2551q(canceled, &profile, Some(&revision)),
            Queued2551QPreparation::Superseded
        ));

        let mut requeued = draft;
        requeued.next_retry_at = Some("2099-01-01T00:00:00Z".to_string());
        assert!(matches!(
            prepare_queued_2551q(requeued, &profile, Some(&revision)),
            Queued2551QPreparation::Superseded
        ));
    }

    #[test]
    fn unresolved_submission_claim_is_never_eligible_for_automatic_retry() {
        let profile = reviewed_profile();
        let mut draft = queued_draft(&profile);
        draft.submission_claim_token = Some("abandoned-network-claim".to_string());
        draft.submission_claimed_at = Some("2099-01-01T00:00:00Z".to_string());

        assert!(queued_2551q_revision(&draft).is_none());
        assert!(matches!(
            prepare_queued_2551q(draft, &profile, None),
            Queued2551QPreparation::Superseded
        ));
    }

    #[test]
    fn queued_1601c_preparation_binds_exact_fields_and_queue_generation() {
        let profile = test_profile();
        let draft = queued_1601c_draft(&profile);
        let revision = match prepare_queued_1601c(draft.clone(), None) {
            Queued1601CPreparation::Ready { draft, revision } => {
                assert!(draft.try_to_bir_xml_payload().is_ok());
                revision
            }
            _ => panic!("the unchanged 1601C queue snapshot should prepare"),
        };

        let mut tampered = draft.clone();
        tampered.tax_14_total_compensation = 10.0;
        match prepare_queued_1601c(tampered, Some(&revision)) {
            Queued1601CPreparation::Rejected { draft, errors, .. } => {
                assert_eq!(draft.status, FilingStatus::Draft);
                assert!(
                    errors
                        .iter()
                        .any(|(field, _)| field == "queued_submission_fingerprint")
                );
            }
            _ => panic!("tampered 1601C fields must fail before XML generation"),
        }

        let mut requeued = draft;
        requeued.next_retry_at = Some("2099-01-01T00:00:00Z".to_string());
        assert!(matches!(
            prepare_queued_1601c(requeued, Some(&revision)),
            Queued1601CPreparation::Superseded
        ));
    }

    #[test]
    fn unresolved_1601c_claim_is_never_eligible_for_automatic_retry() {
        let profile = test_profile();
        let mut draft = queued_1601c_draft(&profile);
        draft.submission_claim_token = Some("unknown-network-outcome".to_string());
        draft.submission_claimed_at = Some("2099-01-01T00:00:00Z".to_string());

        assert!(queued_1601c_revision(&draft).is_none());
        assert!(matches!(
            prepare_queued_1601c(draft, None),
            Queued1601CPreparation::Superseded
        ));
    }

    #[tokio::test]
    async fn process_queued_1601c_success_finalizes_exact_payload_as_sent() {
        let mut profile = test_profile();
        profile.email_tracking_enabled = true;
        profile.imap_email = Some("receipts@example.com".to_string());
        let queued = queued_1601c_draft(&profile);
        let db = Arc::new(Mutex::new(
            Database::open_in_memory_for_tests()
                .expect("the in-memory cron database should initialize"),
        ));
        let id = db
            .lock()
            .expect("the cron database should not be poisoned")
            .save_queued_1601c_draft(&queued)
            .expect("the reviewed 1601C queue snapshot should persist");
        let summary = queued_1601c_summary(id, &queued);
        let transport = RecordingSubmissionTransport::new(TestTransportOutcome::Success);

        process_queued_1601c_with_transport(&summary, &profile, db.clone(), &transport).await;

        let calls = transport.calls();
        assert_eq!(calls.len(), 1);
        assert_exact_1601c_transport_call(&calls[0], &queued);
        let db_guard = db.lock().expect("the cron database should not be poisoned");
        let submitted = db_guard
            .get_1601c_draft(&queued.tin, queued.taxable_year, queued.month)
            .expect("the submitted 1601C lookup should succeed")
            .expect("the submitted 1601C snapshot should remain persisted");
        assert_eq!(submitted.status, FilingStatus::Submitted);
        assert_eq!(submitted.to_bir_field_map(), queued.to_bir_field_map());
        assert_eq!(
            submitted.submission_filename.as_deref(),
            Some(queued.default_submission_filename().as_str())
        );
        assert!(submitted.submitted_at.is_some());
        assert!(submitted.submission_claim_token.is_none());
        assert!(submitted.submission_claimed_at.is_none());
        // Sent is terminal: no confirmation poll is scheduled.
        let jobs = db_guard.list_jobs().expect("the job lookup should succeed");
        assert!(jobs.iter().all(|job| !is_email_poll_job(job)), "{jobs:?}");
    }

    #[tokio::test]
    async fn process_queued_1601c_dry_run_submits_as_sent() {
        let mut profile = test_profile();
        profile.email_tracking_enabled = true;
        profile.imap_email = Some("receipts@example.com".to_string());
        let queued = queued_1601c_draft(&profile);
        let db = Arc::new(Mutex::new(
            Database::open_in_memory_for_tests()
                .expect("the in-memory cron database should initialize"),
        ));
        let id = db
            .lock()
            .expect("the cron database should not be poisoned")
            .save_queued_1601c_draft(&queued)
            .expect("the reviewed 1601C queue snapshot should persist");
        let summary = queued_1601c_summary(id, &queued);
        let transport = RecordingSubmissionTransport::dry_run(TestTransportOutcome::Success);

        process_queued_1601c_with_transport(&summary, &profile, db.clone(), &transport).await;

        let submitted = db
            .lock()
            .expect("the cron database should not be poisoned")
            .get_1601c_draft(&queued.tin, queued.taxable_year, queued.month)
            .expect("the submitted 1601C lookup should succeed")
            .expect("the submitted 1601C snapshot should remain persisted");
        assert_eq!(submitted.status, FilingStatus::Submitted);
        let jobs = db
            .lock()
            .expect("the cron database should not be poisoned")
            .list_jobs()
            .expect("job lookup should succeed");
        assert!(
            jobs.iter().all(|job| !is_email_poll_job(job)),
            "a dry run schedules no poll"
        );
    }

    #[tokio::test]
    async fn process_queued_1601c_unknown_outcome_remains_claimed_and_is_not_retried() {
        let profile = test_profile();
        let queued = queued_1601c_draft(&profile);
        let expected_fingerprint = queued.queued_submission_fingerprint.clone();
        let expected_retry_at = queued.next_retry_at.clone();
        let db = Arc::new(Mutex::new(
            Database::open_in_memory_for_tests()
                .expect("the in-memory cron database should initialize"),
        ));
        let id = db
            .lock()
            .expect("the cron database should not be poisoned")
            .save_queued_1601c_draft(&queued)
            .expect("the reviewed 1601C queue snapshot should persist");
        let summary = queued_1601c_summary(id, &queued);
        let failing_transport = RecordingSubmissionTransport::new(TestTransportOutcome::UnknownIo);

        process_queued_1601c_with_transport(&summary, &profile, db.clone(), &failing_transport)
            .await;

        let calls = failing_transport.calls();
        assert_eq!(calls.len(), 1);
        assert_exact_1601c_transport_call(&calls[0], &queued);
        let claimed = db
            .lock()
            .expect("the cron database should not be poisoned")
            .get_1601c_draft(&queued.tin, queued.taxable_year, queued.month)
            .expect("the claimed 1601C lookup should succeed")
            .expect("the claimed 1601C snapshot should remain persisted");
        assert_eq!(claimed.status, FilingStatus::Queued);
        assert_eq!(claimed.queued_submission_fingerprint, expected_fingerprint);
        assert_eq!(claimed.next_retry_at, expected_retry_at);
        assert_eq!(claimed.submission_attempts, 0);
        assert!(claimed.submission_claim_token.is_some());
        assert!(claimed.submission_claimed_at.is_some());
        assert!(
            claimed
                .submission_error
                .as_deref()
                .is_some_and(|message| message.contains("outcome pending"))
        );
        assert!(queued_1601c_revision(&claimed).is_none());

        let retry_transport = RecordingSubmissionTransport::new(TestTransportOutcome::Success);
        process_queued_1601c_with_transport(&summary, &profile, db.clone(), &retry_transport).await;
        assert!(retry_transport.calls().is_empty());
        let still_claimed = db
            .lock()
            .expect("the cron database should not be poisoned")
            .get_1601c_draft(&queued.tin, queued.taxable_year, queued.month)
            .expect("the unresolved 1601C lookup should succeed")
            .expect("the unresolved 1601C snapshot should remain persisted");
        assert_eq!(
            still_claimed.submission_claim_token,
            claimed.submission_claim_token
        );
        assert!(
            db.lock()
                .expect("the cron database should not be poisoned")
                .list_jobs()
                .expect("the email-poll job lookup should succeed")
                .is_empty()
        );
    }

    // ----- Generic QueueableForm worker -----

    use crate::forms::queueable::test_support::TestForm;

    fn allow_test_form(code: &str) -> Option<&'static str> {
        (code == TestForm::CODE).then_some(TestForm::TYPE)
    }

    fn queued_test_form(db: &Arc<Mutex<Database>>) -> (TestForm, FormDraftSummary) {
        let mut form = TestForm::new(6, 10.0);
        form.queue(crate::filing_queue::QueueAuthSource::Gui)
            .expect("the test form should queue");
        let id = db
            .lock()
            .unwrap()
            .save_queued_queueable(&form)
            .expect("the queued test form should persist");
        let summary = FormDraftSummary {
            id,
            tin: form.tin.clone(),
            form_code: TestForm::CODE.to_string(),
            taxable_year: form.taxable_year,
            quarter: None,
            month: Some(form.month),
            status: FilingStatus::Queued,
            updated_at: form.lifecycle.updated_at.clone(),
        };
        (form, summary)
    }

    fn stored_test_form(db: &Arc<Mutex<Database>>, form: &TestForm) -> TestForm {
        db.lock()
            .unwrap()
            .get_queueable_draft::<TestForm>(&form.tin, form.taxable_year, i64::from(form.month))
            .unwrap()
            .expect("the test form should stay persisted")
    }

    fn test_db() -> Arc<Mutex<Database>> {
        Arc::new(Mutex::new(Database::open_in_memory_for_tests().unwrap()))
    }

    #[tokio::test]
    async fn generic_worker_submits_the_exact_official_payload() {
        let profile = test_profile();
        let db = test_db();
        let (queued, summary) = queued_test_form(&db);
        let transport = RecordingSubmissionTransport::dry_run(TestTransportOutcome::Success);

        process_queued_queueable::<TestForm, _>(
            &summary,
            &profile,
            db.clone(),
            &transport,
            allow_test_form,
        )
        .await;

        let calls = transport.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].form_type, TestForm::TYPE);
        assert_eq!(calls[0].filename, queued.submission_filename());
        assert_eq!(
            String::from_utf8(calls[0].payload.clone()).unwrap(),
            queued.official_payload().unwrap()
        );
        let submitted = stored_test_form(&db, &queued);
        assert_eq!(submitted.lifecycle.status, FilingStatus::Submitted);
        assert!(submitted.lifecycle.is_unclaimed());
        assert_eq!(
            submitted.lifecycle.submission_filename.as_deref(),
            Some(queued.submission_filename().as_str())
        );
    }

    #[tokio::test]
    async fn generic_receipt_confirms_by_uploaded_filename_when_the_period_cannot_key_it() {
        let profile = test_profile();
        let db = test_db();
        let (queued, summary) = queued_test_form(&db);
        let transport = RecordingSubmissionTransport::dry_run(TestTransportOutcome::Success);
        process_queued_queueable::<TestForm, _>(
            &summary,
            &profile,
            db.clone(),
            &transport,
            allow_test_form,
        )
        .await;

        // BIR echoes the base filename; an event-based form's period part
        // (a date, a TCT) does not parse back to the draft's key.
        let local_now =
            chrono::Utc::now() + chrono::Duration::hours(8) + chrono::Duration::minutes(1);
        let uploaded = queued.submission_filename();
        let confirmation = crate::receipt::BirReceiptConfirmation {
            filename: format!("{}.xml", uploaded.split('#').next().unwrap()),
            date_received: local_now.date_naive(),
            time_received: local_now.time(),
            source_from: Some("ebirforms-noreply@bir.gov.ph".to_string()),
            raw_text: "This confirms receipt".to_string(),
            raw_html: None,
        };
        let guard = db.lock().unwrap();
        let (mut saved, _) = guard.save_submission_receipt(&confirmation).unwrap();
        saved.period = "03152025_T123456".to_string();
        assert_eq!(
            guard
                .confirm_queueable_from_receipt::<TestForm>(&saved)
                .unwrap(),
            crate::db::ReceiptConfirmationOutcome::Confirmed
        );
        drop(guard);
        assert_eq!(
            stored_test_form(&db, &queued).lifecycle.status,
            FilingStatus::Confirmed
        );
    }

    #[tokio::test]
    async fn generic_worker_pre_store_failure_stays_unclaimed_and_backs_off() {
        let profile = test_profile();
        let db = test_db();
        let (_queued, summary) = queued_test_form(&db);
        let transport = RecordingSubmissionTransport::new(TestTransportOutcome::PreStore);

        process_queued_queueable::<TestForm, _>(
            &summary,
            &profile,
            db.clone(),
            &transport,
            allow_test_form,
        )
        .await;

        assert!(transport.calls().is_empty());
        let failed = stored_test_form(&db, &_queued);
        assert_eq!(failed.lifecycle.status, FilingStatus::Queued);
        assert_eq!(failed.lifecycle.submission_attempts, 1);
        assert!(failed.lifecycle.is_unclaimed());
        assert!(
            failed
                .lifecycle
                .submission_error
                .as_deref()
                .is_some_and(|message| message.contains("no PUT attempted"))
        );
    }

    #[tokio::test]
    async fn generic_worker_unknown_outcome_stays_claimed_and_is_not_retried() {
        let profile = test_profile();
        let db = test_db();
        let (queued, summary) = queued_test_form(&db);
        let transport = RecordingSubmissionTransport::new(TestTransportOutcome::UnknownIo);

        process_queued_queueable::<TestForm, _>(
            &summary,
            &profile,
            db.clone(),
            &transport,
            allow_test_form,
        )
        .await;
        assert_eq!(transport.calls().len(), 1);
        let claimed = stored_test_form(&db, &queued);
        assert_eq!(claimed.lifecycle.status, FilingStatus::Queued);
        assert!(claimed.lifecycle.submission_put_started_at.is_some());
        assert!(!claimed.lifecycle.is_unclaimed());

        // A second run must not PUT again: the outcome is unknown.
        let retry = RecordingSubmissionTransport::new(TestTransportOutcome::Success);
        process_queued_queueable::<TestForm, _>(
            &summary,
            &profile,
            db.clone(),
            &retry,
            allow_test_form,
        )
        .await;
        assert!(retry.calls().is_empty());
    }

    #[tokio::test]
    async fn generic_worker_refuses_a_form_the_registry_does_not_authorize() {
        let profile = test_profile();
        let db = test_db();
        let (queued, summary) = queued_test_form(&db);
        let transport = RecordingSubmissionTransport::new(TestTransportOutcome::Success);

        process_queued_queueable::<TestForm, _>(
            &summary,
            &profile,
            db.clone(),
            &transport,
            crate::forms::queue_authorized_form_type_id,
        )
        .await;

        assert!(transport.calls().is_empty());
        assert_eq!(
            stored_test_form(&db, &queued).lifecycle.status,
            FilingStatus::Queued
        );
    }

    #[test]
    fn generic_cancel_returns_to_draft_and_requeue_replaces_it() {
        let db = test_db();
        let (queued, _summary) = queued_test_form(&db);
        let canceled = db.lock().unwrap().cancel_queued_queueable(&queued).unwrap();
        assert_eq!(canceled.lifecycle.status, FilingStatus::Draft);
        assert_eq!(
            stored_test_form(&db, &queued).lifecycle.status,
            FilingStatus::Draft
        );

        let mut edited = canceled;
        edited.amount = 12.0;
        db.lock().unwrap().save_queueable_draft(&edited).unwrap();
        edited
            .queue(crate::filing_queue::QueueAuthSource::Gui)
            .unwrap();
        db.lock().unwrap().save_queued_queueable(&edited).unwrap();
        let requeued = stored_test_form(&db, &queued);
        assert_eq!(requeued.lifecycle.status, FilingStatus::Queued);
        assert_eq!(requeued.amount, 12.0);

        // A queued snapshot cannot be overwritten by a draft save.
        let mut stale = requeued.clone();
        stale.lifecycle.revert_to_draft();
        assert!(db.lock().unwrap().save_queueable_draft(&stale).is_err());
    }

    #[tokio::test]
    async fn process_queued_1601c_pre_store_failure_stays_unclaimed_and_can_retry() {
        let profile = test_profile();
        let queued = queued_1601c_draft(&profile);
        let expected_fingerprint = queued.queued_submission_fingerprint.clone();
        let db = Arc::new(Mutex::new(
            Database::open_in_memory_for_tests()
                .expect("the in-memory cron database should initialize"),
        ));
        let id = db
            .lock()
            .expect("the cron database should not be poisoned")
            .save_queued_1601c_draft(&queued)
            .expect("the reviewed 1601C queue snapshot should persist");
        let summary = queued_1601c_summary(id, &queued);
        let failing_transport = RecordingSubmissionTransport::new(TestTransportOutcome::PreStore);

        process_queued_1601c_with_transport(&summary, &profile, db.clone(), &failing_transport)
            .await;

        assert!(failing_transport.calls().is_empty());
        let failed = db
            .lock()
            .expect("the cron database should not be poisoned")
            .get_1601c_draft(&queued.tin, queued.taxable_year, queued.month)
            .expect("the unclaimed 1601C lookup should succeed")
            .expect("the unclaimed 1601C snapshot should remain persisted");
        assert_eq!(failed.status, FilingStatus::Queued);
        assert_eq!(failed.queued_submission_fingerprint, expected_fingerprint);
        assert_eq!(failed.submission_attempts, 1);
        assert!(failed.submission_claim_token.is_none());
        assert!(failed.submission_claimed_at.is_none());
        assert!(
            failed
                .submission_error
                .as_deref()
                .is_some_and(|message| message.contains("no PUT attempted"))
        );
        assert!(queued_1601c_revision(&failed).is_some());

        let expected_retry_at = failed.next_retry_at.clone();
        let mut retryable = failed.clone();
        retryable.next_retry_at = Some("1999-01-01T00:00:00Z".to_string());
        assert!(
            db.lock()
                .expect("the cron database should not be poisoned")
                .replace_unclaimed_queued_1601c_submission(
                    &retryable,
                    &expected_fingerprint,
                    &expected_retry_at,
                    1,
                )
                .expect("forcing the retry timestamp should CAS-succeed")
        );

        let retry_transport = RecordingSubmissionTransport::new(TestTransportOutcome::Success);
        process_queued_1601c_with_transport(&summary, &profile, db.clone(), &retry_transport).await;
        assert_eq!(retry_transport.calls().len(), 1);
        let submitted = db
            .lock()
            .expect("the cron database should not be poisoned")
            .get_1601c_draft(&queued.tin, queued.taxable_year, queued.month)
            .expect("the submitted 1601C lookup should succeed")
            .expect("the submitted 1601C snapshot should remain persisted");
        assert_eq!(submitted.status, FilingStatus::Submitted);
        assert!(submitted.submission_claim_token.is_none());
        assert!(submitted.submission_claimed_at.is_none());
    }

    #[test]
    fn submission_queue_uses_the_local_year_at_the_manila_boundary() {
        let utc = Utc
            .with_ymd_and_hms(2099, 12, 31, 16, 30, 0)
            .single()
            .expect("the UTC test instant must be valid");
        let manila =
            chrono::FixedOffset::east_opt(8 * 60 * 60).expect("the Manila offset must be valid");
        let local = utc.with_timezone(&manila);

        assert_eq!(utc.year(), 2099);
        assert_eq!(local.year(), 2100);
        assert_eq!(submission_queue_year_at(&local), 2100);
    }

    #[tokio::test]
    async fn expired_unstarted_1601c_claim_recovers_and_retries() {
        let profile = test_profile();
        let queued = queued_1601c_draft(&profile);
        let fingerprint = queued.queued_submission_fingerprint.clone();
        let retry = queued.next_retry_at.clone();
        let db = Arc::new(Mutex::new(
            Database::open_in_memory_for_tests()
                .expect("the in-memory cron database should initialize"),
        ));
        let id = db
            .lock()
            .expect("the cron database should not be poisoned")
            .save_queued_1601c_draft(&queued)
            .expect("queue");
        let summary = queued_1601c_summary(id, &queued);
        let claim = db
            .lock()
            .expect("the cron database should not be poisoned")
            .claim_queued_1601c_submission(
                &queued.tin,
                queued.taxable_year,
                queued.month,
                &fingerprint,
                &retry,
                0,
            )
            .expect("claim");
        assert!(matches!(
            claim,
            crate::db::Claim1601CSubmissionResult::Claimed { .. }
        ));

        let future = Utc::now() + chrono::Duration::hours(1);
        assert!(
            db.lock()
                .expect("the cron database should not be poisoned")
                .recover_expired_unstarted_1601c_claim(
                    &queued.tin,
                    queued.taxable_year,
                    queued.month,
                    future,
                )
                .expect("recover")
        );
        let recovered = db
            .lock()
            .expect("the cron database should not be poisoned")
            .get_1601c_draft(&queued.tin, queued.taxable_year, queued.month)
            .unwrap()
            .unwrap();
        assert!(recovered.submission_claim_token.is_none());
        assert_eq!(recovered.submission_attempts, 0);
        assert!(recovered.queue_authorization.is_some());

        let transport = RecordingSubmissionTransport::new(TestTransportOutcome::Success);
        process_queued_1601c_with_transport(&summary, &profile, db.clone(), &transport).await;
        assert_eq!(transport.calls().len(), 1);
        let submitted = db
            .lock()
            .expect("the cron database should not be poisoned")
            .get_1601c_draft(&queued.tin, queued.taxable_year, queued.month)
            .unwrap()
            .unwrap();
        assert_eq!(submitted.status, FilingStatus::Submitted);
    }

    #[tokio::test]
    async fn put_started_claim_does_not_recover() {
        let profile = test_profile();
        let queued = queued_1601c_draft(&profile);
        let fingerprint = queued.queued_submission_fingerprint.clone();
        let retry = queued.next_retry_at.clone();
        let db = Arc::new(Mutex::new(
            Database::open_in_memory_for_tests()
                .expect("the in-memory cron database should initialize"),
        ));
        db.lock()
            .expect("the cron database should not be poisoned")
            .save_queued_1601c_draft(&queued)
            .unwrap();
        let claim = db
            .lock()
            .expect("the cron database should not be poisoned")
            .claim_queued_1601c_submission(
                &queued.tin,
                queued.taxable_year,
                queued.month,
                &fingerprint,
                &retry,
                0,
            )
            .unwrap();
        let crate::db::Claim1601CSubmissionResult::Claimed { token, .. } = claim else {
            panic!("expected claim");
        };
        assert!(
            db.lock()
                .expect("the cron database should not be poisoned")
                .mark_claimed_1601c_put_started(
                    &queued.tin,
                    queued.taxable_year,
                    queued.month,
                    &token,
                )
                .unwrap()
        );
        let future = Utc::now() + chrono::Duration::hours(1);
        assert!(
            !db.lock()
                .expect("the cron database should not be poisoned")
                .recover_expired_unstarted_1601c_claim(
                    &queued.tin,
                    queued.taxable_year,
                    queued.month,
                    future,
                )
                .unwrap()
        );
    }

    #[tokio::test]
    async fn mandatory_fixture_dry_run_transport_submits() {
        let mut profile = crate::filing_queue::mandatory_lab_1601c_profile();
        profile.email_tracking_enabled = true;
        profile.imap_email = Some("taxpayer@example.com".to_string());
        let mut queued = crate::filing_queue::mandatory_lab_1601c_draft();
        queued.transition_to_queued().unwrap();
        let db = Arc::new(Mutex::new(
            Database::open_in_memory_for_tests()
                .expect("the in-memory cron database should initialize"),
        ));
        let id = db
            .lock()
            .expect("the cron database should not be poisoned")
            .save_queued_1601c_draft(&queued)
            .unwrap();
        let summary = queued_1601c_summary(id, &queued);
        let transport = RecordingSubmissionTransport::dry_run(TestTransportOutcome::Success);
        process_queued_1601c_with_transport(&summary, &profile, db.clone(), &transport).await;
        assert_eq!(transport.calls().len(), 1);
        assert_eq!(
            transport.calls()[0].filename,
            "00000000000000-1601Cv2018-092026#taxpayer@example.com#.xml"
        );
        let submitted = db
            .lock()
            .expect("the cron database should not be poisoned")
            .get_1601c_draft(&queued.tin, queued.taxable_year, queued.month)
            .unwrap()
            .unwrap();
        assert_eq!(submitted.status, FilingStatus::Submitted);
        assert_eq!(submitted.tax_36_total_amount_payable, 0.0);
        assert!(
            db.lock()
                .expect("the cron database should not be poisoned")
                .list_jobs()
                .unwrap()
                .iter()
                .all(|job| !is_email_poll_job(job)),
            "a dry run schedules no poll"
        );
    }
}
