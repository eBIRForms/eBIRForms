use crate::bir_sign::SeenSign;
use crate::db::{Claim1601CSubmissionResult, Claim2551QSubmissionResult, Database};
use crate::forms::form_1601c::Form1601CDraft;
use crate::forms::form_2551q::Form2551QDraft;
use crate::forms::queueable::{QueueableForm, QueueableKind};
use crate::forms::{FilingStatus, FormDraftSummary};
use crate::profile::TaxpayerProfile;
use crate::smart_queue::{
    self, Blame, DeviceTries, Ending, PathBackoff, QueueMode, QueuePolicy, QueueRng, QueueState,
    Retry, TryClass, Waiting,
};
use chrono::{DateTime, Utc};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use tokio::sync::{Notify, mpsc};
use tracing::{error, info, warn};

static WAKE_TX: OnceLock<mpsc::Sender<()>> = OnceLock::new();
static ACTIVE_JOBS: OnceLock<Arc<Mutex<HashSet<String>>>> = OnceLock::new();
/// Set by [`request_shutdown`]. The cron loop stops, cutting short a tick
/// that is waiting on the network, so the submission worker can drop the
/// database and its lease.
static SHUTDOWN: AtomicBool = AtomicBool::new(false);
/// Rung by [`request_shutdown`]: interrupts a tick in progress.
static SHUTDOWN_RING: Notify = Notify::const_new();
/// The longest one send may take, from the session to the result. One that
/// takes longer is left as "BIR may have it": it is never sent again by
/// itself, and the queue goes on.
pub const SEND_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10 * 60);
/// The longest opening a session may take (it talks to Buwiz only).
pub const SESSION_DEADLINE: std::time::Duration = std::time::Duration::from_secs(2 * 60);
/// What a send that ran past [`SEND_DEADLINE`] stores as its reason.
pub const SEND_TIMED_OUT: &str = "the send did not finish within 10 minutes, so BIR may have received this return; check for its confirmation email before sending it again";
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

/// The clock and the random source the queue decides by. The app runs on
/// [`QueueEnv::system`]; tests and simulations inject a clock and a seeded
/// source ([`smart_queue::SeededRng`]), which makes every decision
/// repeatable.
pub struct QueueEnv {
    clock: Box<dyn Fn() -> DateTime<Utc> + Send + Sync>,
    rng: Mutex<Box<dyn QueueRng + Send>>,
    send_deadline: std::time::Duration,
    session_deadline: std::time::Duration,
}

impl QueueEnv {
    /// The wall clock and the operating system's random source.
    pub fn system() -> Self {
        Self::new(Utc::now, smart_queue::OsRng)
    }

    pub fn new(
        clock: impl Fn() -> DateTime<Utc> + Send + Sync + 'static,
        rng: impl QueueRng + Send + 'static,
    ) -> Self {
        Self {
            clock: Box::new(clock),
            rng: Mutex::new(Box::new(rng)),
            send_deadline: SEND_DEADLINE,
            session_deadline: SESSION_DEADLINE,
        }
    }

    /// The same env with other deadlines for a send and a session (tests
    /// of a hung send).
    pub fn with_deadlines(
        mut self,
        send: std::time::Duration,
        session: std::time::Duration,
    ) -> Self {
        self.send_deadline = send;
        self.session_deadline = session;
        self
    }

    pub fn now(&self) -> DateTime<Utc> {
        (self.clock)()
    }

    fn with_rng<R>(&self, use_it: impl FnOnce(&mut dyn QueueRng) -> R) -> R {
        let mut rng = self
            .rng
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        use_it(&mut **rng)
    }
}

/// What one send needs to plan the retry of a try that did not land: the
/// time, the numbers, whether the device guard trips on a counted failure
/// now, and the queue-wide back-off a path failure moves. It remembers
/// where the last try failed, for the tick.
struct Planner<'a> {
    env: &'a QueueEnv,
    policy: QueuePolicy,
    slowed_down: bool,
    path: Mutex<PathBackoff>,
    last_blame: Mutex<Option<Blame>>,
}

impl<'a> Planner<'a> {
    #[cfg(test)]
    fn new(env: &'a QueueEnv, policy: QueuePolicy, slowed_down: bool) -> Self {
        Self::with_path(env, policy, slowed_down, PathBackoff::default())
    }

    fn with_path(
        env: &'a QueueEnv,
        policy: QueuePolicy,
        slowed_down: bool,
        path: PathBackoff,
    ) -> Self {
        Self {
            env,
            policy,
            slowed_down,
            path: Mutex::new(path),
            last_blame: Mutex::new(None),
        }
    }

    fn now(&self) -> DateTime<Utc> {
        self.env.now()
    }

    /// The retry of a return in `state` whose try ended now as `ending`.
    fn retry(&self, state: &QueueState, ending: Ending<'_>) -> Retry {
        let now = self.now();
        if let Ending::Failed { blame, .. } = ending
            && let Ok(mut last) = self.last_blame.lock()
        {
            *last = Some(blame);
        }
        let mut path = self
            .path
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.env.with_rng(|rng| {
            smart_queue::plan(
                state,
                ending,
                self.slowed_down,
                now,
                now,
                &mut path,
                &self.policy,
                rng,
            )
        })
    }

    /// Where the last counted try failed, if one did.
    fn last_blame(&self) -> Option<Blame> {
        self.last_blame.lock().ok().and_then(|last| *last)
    }

    fn path(&self) -> PathBackoff {
        self.path
            .lock()
            .map(|path| path.clone())
            .unwrap_or_default()
    }

    /// `send`, or "BIR may have it" when it runs past the send deadline.
    async fn within_deadline(
        &self,
        send: impl Future<Output = Result<StoreReport, StoreFailure>>,
    ) -> Result<StoreReport, StoreFailure> {
        match tokio::time::timeout(self.env.send_deadline, send).await {
            Ok(result) => result,
            Err(_) => Err(StoreFailure::MaybeSent(TransportError(
                SEND_TIMED_OUT.to_owned(),
            ))),
        }
    }

    /// `open`, or a failed session when it runs past the session deadline.
    async fn session<S>(
        &self,
        open: impl Future<Output = Result<S, TransportError>>,
    ) -> Result<S, TransportError> {
        tokio::time::timeout(self.env.session_deadline, open)
            .await
            .unwrap_or_else(|_| {
                Err(TransportError(
                    "the session did not open within 2 minutes".to_owned(),
                ))
            })
    }
}

/// What a failed session (not a final refusal) means, for [`Planner::retry`].
fn session_ending(class: TryClass, reason: &str) -> Ending<'_> {
    match class {
        TryClass::NotATry | TryClass::Refused | TryClass::MaybeSent => {
            Ending::NotATry { wait_until: None }
        }
        TryClass::Answered => Ending::Answered { reason },
        TryClass::Counted => Ending::Failed {
            blame: Blame::Upload,
            reason,
        },
        TryClass::NoService => Ending::Failed {
            blame: Blame::NoService,
            reason,
        },
    }
}

/// How the one send of a tick ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendOutcome {
    /// The return is Sent.
    Sent,
    /// It did not land; the class says what that means.
    Failed(TryClass),
    /// Nothing was tried (not due, changed meanwhile, invalid, claimed).
    NotTried,
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
    // A permit, so a loop that is not waiting yet still sees it.
    SHUTDOWN_RING.notify_one();
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

/// [`wake`] without a warning when no worker runs in this process (a one-shot
/// sender, a switch flipped before the worker starts).
pub fn wake_quietly() {
    let _ = wake_with_status();
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
    let env = QueueEnv::system();

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
        // A shutdown cuts the tick short: a send it interrupts is claimed
        // with its PUT started, so it is never sent again by itself.
        let stopped = run_tick_until(&SHUTDOWN_RING, db.clone(), &transport, &env)
            .await
            .is_none();
        if stopped || shutdown_requested() {
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

/// One tick, unless `stop` rings first: `None` when it was cut short.
pub(crate) async fn run_tick_until<T: SubmissionTransport + 'static>(
    stop: &Notify,
    db: Arc<Mutex<Database>>,
    transport: &Arc<T>,
    env: &QueueEnv,
) -> Option<Option<SendOutcome>> {
    tokio::select! {
        outcome = run_queue_tick_with(db, transport, env) => Some(outcome),
        _ = stop.notified() => {
            warn!("Cron: shutdown interrupted the queue tick");
            None
        }
    }
}

/// Runs a single iteration of the background job queue: at most one
/// return, the head of the line across every TIN, on the wall clock
/// ([`run_queue_tick_with`]). Extracted from `start_cron_jobs` to allow
/// deterministic unit testing without being blocked by an infinite loop or
/// thread sleep.
pub async fn run_queue_tick<T: SubmissionTransport + 'static>(
    db: Arc<Mutex<Database>>,
    transport: &Arc<T>,
) {
    run_queue_tick_with(db, transport, &QueueEnv::system()).await;
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

/// A queued return whose last reason is a TIN refusal (left Queued by a
/// build that retried those) is due now, whatever its backoff says: the
/// server decides whether the TIN can send yet, and a refusal now sends it
/// back to Draft at once.
fn previous_tin_refusal(error: Option<&str>) -> bool {
    error.and_then(TinRefusal::from_reason).is_some()
}

/// The most characters of a refusal's text the queue stores and logs,
/// before its code: a server that put long free text there cannot fill the
/// row or the log.
const REFUSAL_REASON_CAP: usize = 300;

/// The reason stored for a TIN refusal: `prefix` and the error's text, cut
/// to [`REFUSAL_REASON_CAP`], ending with the code.
fn refusal_reason(prefix: &str, error: &TransportError) -> String {
    TinRefusal::capped(&format!("{prefix}{error}"), REFUSAL_REASON_CAP)
}

/// Why a transport call failed. The queue only needs the message: before
/// PUT a failure is retried, after PUT the outcome is unknown.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct TransportError(pub String);

pub use crate::tin_refusal::TinRefusal;

impl TransportError {
    /// The TIN refusal this error's text ends with, if any
    /// ([`TinRefusal::from_reason`]).
    pub fn refusal(&self) -> Option<TinRefusal> {
        TinRefusal::from_reason(&self.0)
    }
}

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

    /// [`Self::store_session`] that also says whether a failure reached BIR
    /// and hands back the receipt. The default knows neither, so every
    /// failure counts as [`StoreFailure::MaybeSent`]. Every queued send
    /// (queueable forms, 1601C and 2551Q) uses this.
    fn store_session_reported<'a>(
        &'a self,
        session: Self::Session,
        filename: &'a str,
        payload: &'a [u8],
    ) -> Pin<Box<dyn Future<Output = Result<StoreReport, StoreFailure>> + Send + 'a>> {
        Box::pin(async move {
            self.store_session(session, filename, payload)
                .await
                .map(|()| StoreReport::default())
                .map_err(StoreFailure::MaybeSent)
        })
    }

    /// What a failed [`Self::open_session`] (not a TIN refusal) means for
    /// the queue. The default counts it, as a transport whose session opens
    /// a connection toward BIR must; one whose session only talks to its own
    /// server (sign-in, keys) says [`TryClass::NotATry`].
    fn session_failure_class(&self, _error: &TransportError) -> TryClass {
        TryClass::Counted
    }

    /// The newest BIR sign the transport heard. The default hears none,
    /// and without a sign the queue runs on its own schedule alone.
    fn sign(&self) -> Option<SeenSign> {
        None
    }
}

/// What a transport learned from a PUT that worked.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StoreReport {
    /// The submission service's signed receipt, kept on the return.
    pub receipt: Option<String>,
    /// Nothing reached BIR: the far side only simulated the PUT.
    pub dry_run: bool,
}

/// A PUT that did not end in a receipt.
#[derive(Debug, thiserror::Error)]
pub enum StoreFailure {
    /// Nothing reached BIR: the queue retries with backoff.
    #[error("{0}")]
    NotSent(TransportError),
    /// BIR may have the return: it is never sent again automatically.
    #[error("{0}")]
    MaybeSent(TransportError),
    /// Nothing reached BIR and it was not a try: a hold, a failure of class
    /// `ours`, signed out, the server unreachable. The return waits the
    /// first wait without a count, and never before `wait_until`.
    #[error("{error}")]
    NotATry {
        error: TransportError,
        wait_until: Option<DateTime<Utc>>,
    },
    /// Nothing reached BIR's mailbox and the try counts; `blame` says
    /// where it failed, which decides who waits: the whole queue for a try
    /// that never got past the front desk ([`Blame::Path`]), this return
    /// alone for the rest. [`StoreFailure::NotSent`] is this with
    /// [`Blame::Upload`].
    #[error("{error}")]
    Counted { error: TransportError, blame: Blame },
}

impl StoreFailure {
    /// The error, whatever the outcome.
    pub fn error(&self) -> &TransportError {
        match self {
            Self::NotSent(error)
            | Self::MaybeSent(error)
            | Self::NotATry { error, .. }
            | Self::Counted { error, .. } => error,
        }
    }
}

/// How the queue's reason for a [`StoreFailure::NotSent`] attempt begins.
pub(crate) const NOT_SENT_PREFIX: &str = "Not sent (nothing reached BIR): ";

/// How the queue's reason for a session that failed before any PUT begins.
const SESSION_FAILED_PREFIX: &str = "Submission session failed before upload (no PUT attempted): ";

/// The in-flight job key of a queued return in [`get_active_jobs`]: held
/// while this process's worker runs that return's send.
pub fn queue_job_key(summary: &FormDraftSummary) -> String {
    format!(
        "form:{}:{}:{}:{}",
        summary.form_code,
        summary.tin,
        summary.taxable_year,
        summary.month.unwrap_or(summary.quarter.unwrap_or(0))
    )
}

async fn process_queued_1601c_in<T: SubmissionTransport>(
    summary: &FormDraftSummary,
    profile: &TaxpayerProfile,
    db: Arc<Mutex<Database>>,
    transport: &T,
    planner: &Planner<'_>,
) -> SendOutcome {
    let Some(month) = summary.month else {
        warn!("Cron: Refusing 1601C queue row without a monthly period");
        return SendOutcome::NotTried;
    };
    let loaded_draft = {
        let db_guard = match db.lock() {
            Ok(guard) => guard,
            Err(_) => return SendOutcome::NotTried,
        };
        match db_guard.get_1601c_draft(&summary.tin, summary.taxable_year, month) {
            Ok(Some(draft)) => draft,
            _ => return SendOutcome::NotTried,
        }
    };

    let loaded_draft = if loaded_draft.submission_claim_token.is_some()
        || loaded_draft.submission_claimed_at.is_some()
    {
        let recovered = {
            let db_guard = match db.lock() {
                Ok(guard) => guard,
                Err(_) => return SendOutcome::NotTried,
            };
            db_guard
                .recover_expired_unstarted_1601c_claim(
                    &loaded_draft.tin,
                    loaded_draft.taxable_year,
                    loaded_draft.month,
                    planner.now(),
                )
                .unwrap_or(false)
        };
        if !recovered {
            warn!(
                "Cron: 1601C {} has an unresolved submission outcome; automatic retry is disabled and support-assisted manual reconciliation is required",
                loaded_draft.period_code()
            );
            return SendOutcome::NotTried;
        }
        let recovered_draft = {
            let db_guard = match db.lock() {
                Ok(guard) => guard,
                Err(_) => return SendOutcome::NotTried,
            };
            match db_guard.get_1601c_draft(&summary.tin, summary.taxable_year, month) {
                Ok(Some(draft)) => draft,
                _ => return SendOutcome::NotTried,
            }
        };
        if recovered_draft.submission_claim_token.is_some()
            || recovered_draft.submission_claimed_at.is_some()
        {
            return SendOutcome::NotTried;
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
            return SendOutcome::NotTried;
        }
        Queued1601CPreparation::Superseded => return SendOutcome::NotTried,
    };

    match queued_retry_is_due(draft.next_retry_at.as_deref(), planner.now()) {
        Ok(false) if !previous_tin_refusal(draft.submission_error.as_deref()) => {
            return SendOutcome::NotTried;
        }
        Ok(_) => {}
        Err(error) => {
            // Never Draft for a bad time: the tick stamps a real one, and
            // the return goes when that comes.
            warn!(
                "Cron: queued 1601C {} has an unreadable retry time ({}); it waits for the queue to stamp a real one",
                draft.period_code(),
                error
            );
            return SendOutcome::NotTried;
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
            return SendOutcome::NotTried;
        }
    };
    let Some(form_type) = crate::forms::queue_authorized_form_type_id("1601C") else {
        warn!(
            "Cron: Refusing to submit {} because 1601C is not authorized for queue submission",
            draft.period_code()
        );
        return SendOutcome::NotTried;
    };

    let session = match planner
        .session(transport.open_session(form_type, &draft.tin))
        .await
    {
        Ok(session) => session,
        Err(error) if smart_queue::is_final_reason(&error.0) => {
            refuse_unclaimed_1601c(
                &mut draft,
                &queue_revision,
                &db,
                profile,
                refusal_reason(SESSION_FAILED_PREFIX, &error),
            );
            return SendOutcome::Failed(TryClass::Refused);
        }
        Err(error) => {
            let class = transport.session_failure_class(&error);
            fail_draft_1601c(
                &mut draft,
                &queue_revision,
                db.clone(),
                format!("{SESSION_FAILED_PREFIX}{error}"),
                planner,
                class,
            );
            return SendOutcome::Failed(class);
        }
    };

    // Claim only after the session is open, immediately before PUT.
    // Connect/login timeouts stay unclaimed and use the existing retry CAS.
    let claim_result = {
        let db_guard = match db.lock() {
            Ok(guard) => guard,
            Err(_) => return SendOutcome::NotTried,
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
            return SendOutcome::NotTried;
        }
        Ok(Claim1601CSubmissionResult::Superseded) => {
            info!(
                "Cron: 1601C submission job for {} was canceled or superseded before PUT",
                draft.period_code()
            );
            return SendOutcome::NotTried;
        }
        Err(error) => {
            warn!(
                "Cron: Could not claim queued 1601C {} because the database claim failed: {}",
                draft.period_code(),
                error
            );
            return SendOutcome::NotTried;
        }
    };

    let dry_run = transport.is_dry_run();
    let put_marked = {
        let db_guard = match db.lock() {
            Ok(guard) => guard,
            Err(_) => return SendOutcome::NotTried,
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
        return SendOutcome::NotTried;
    }
    let state_before = draft.smart_queue.clone();
    match planner
        .within_deadline(transport.store_session_reported(session, &filename, xml_payload.as_bytes()))
        .await
    {
        Ok(report) => {
            let dry_run = dry_run || report.dry_run;
            draft.submission_receipt = report.receipt;
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
            SendOutcome::Sent
        }
        Err(failure) if is_final(&failure) => {
            let reason = refusal_reason(NOT_SENT_PREFIX, failure.error());
            warn!(
                "Cron: 1601C {} was refused for good (nothing reached BIR); back to Draft: {}",
                draft.period_code(),
                reason
            );
            if let Ok(db_guard) = db.lock() {
                match db_guard.refuse_claimed_1601c_tin(
                    &draft.tin,
                    draft.taxable_year,
                    draft.month,
                    &claim_token,
                    reason.clone(),
                ) {
                    Ok(true) => record_refusal_alert(
                        &db_guard,
                        profile,
                        "1601C",
                        &monthly_period_label(draft.taxable_year, draft.month),
                        &reason,
                    ),
                    Ok(false) => info!(
                        "Cron: 1601C {} claim changed before its refusal was recorded",
                        draft.period_code()
                    ),
                    Err(db_error) => {
                        warn!("Cron: could not record the refused 1601C send: {db_error}")
                    }
                }
            }
            SendOutcome::Failed(TryClass::Refused)
        }
        Err(
            failure @ (StoreFailure::NotSent(_)
            | StoreFailure::NotATry { .. }
            | StoreFailure::Counted { .. }),
        ) => {
            let (ending, class) = not_sent_ending(&failure);
            let error = failure.error();
            let retry = planner.retry(&state_before, ending);
            log_not_sent("1601C", &draft.period_code(), error, &retry);
            if let Ok(db_guard) = db.lock()
                && let Err(db_error) = db_guard.fail_claimed_1601c_not_sent(
                    &draft.tin,
                    draft.taxable_year,
                    draft.month,
                    &claim_token,
                    format!("{NOT_SENT_PREFIX}{error}"),
                    &retry,
                )
            {
                warn!("Cron: could not record the failed 1601C send: {db_error}");
            }
            SendOutcome::Failed(class)
        }
        Err(StoreFailure::MaybeSent(error)) => {
            warn!(
                %error,
                "Cron: 1601C PUT ended after the network claim; outcome is unknown and support-assisted manual reconciliation is required"
            );
            SendOutcome::Failed(TryClass::MaybeSent)
        }
    }
}

/// [`process_queued_1601c_in`] on the wall clock and the default numbers,
/// as the worker tests call it.
#[cfg(test)]
async fn process_queued_1601c_with_transport<T: SubmissionTransport>(
    summary: &FormDraftSummary,
    profile: &TaxpayerProfile,
    db: Arc<Mutex<Database>>,
    transport: &T,
) {
    let env = QueueEnv::system();
    let planner = Planner::new(&env, QueuePolicy::default(), false);
    process_queued_1601c_in(summary, profile, db, transport, &planner).await;
}

/// A store failure that reached nothing at BIR: what it means for the
/// queue, its class for the tick, and its error. A plain `NotSent` is the
/// return's own failure after the upload, as before the queue knew more.
fn not_sent_ending(failure: &StoreFailure) -> (Ending<'_>, TryClass) {
    match failure {
        StoreFailure::NotATry { wait_until, .. } => (
            Ending::NotATry {
                wait_until: *wait_until,
            },
            TryClass::NotATry,
        ),
        StoreFailure::Counted { error, blame } => (
            Ending::Failed {
                blame: *blame,
                reason: &error.0,
            },
            blame.class(),
        ),
        StoreFailure::NotSent(error) | StoreFailure::MaybeSent(error) => (
            Ending::Failed {
                blame: Blame::Upload,
                reason: &error.0,
            },
            TryClass::Counted,
        ),
    }
}

/// A store failure whose reason ends in a code no retry changes (a TIN
/// refusal, `return_mismatch`): back to Draft at once, whatever its class.
fn is_final(failure: &StoreFailure) -> bool {
    matches!(
        failure,
        StoreFailure::NotSent(_) | StoreFailure::Counted { .. } | StoreFailure::NotATry { .. }
    ) && smart_queue::is_final_reason(&failure.error().0)
}

/// A queued return that did not reach BIR stays queued: say when it goes
/// again, so a long outage reads as waiting, not as a stopped queue.
fn log_not_sent(form_code: &str, period: &str, error: &TransportError, retry: &Retry) {
    warn!(
        "Cron: {} {} was not sent (nothing reached BIR{}): {}; it stays queued after {} counted failures, next try at {}",
        form_code,
        period,
        if retry.was_a_try() { "" } else { ", not a try" },
        error,
        retry.failures,
        retry.at.to_rfc3339()
    );
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
    planner: &Planner<'_>,
    class: TryClass,
) {
    let retry = planner.retry(
        &draft.lifecycle().smart_queue,
        session_ending(class, &error_message),
    );
    warn!(
        "Cron: {} submission preparation failed for {}{}: {}; next try at {}",
        F::FORM_CODE,
        draft.period_code(),
        if retry.was_a_try() { "" } else { " (not a try)" },
        error_message,
        retry.at.to_rfc3339()
    );
    draft
        .lifecycle_mut()
        .record_failed_try(error_message, &retry);
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
async fn process_queued_queueable_in<F: QueueableForm, T: SubmissionTransport>(
    summary: &FormDraftSummary,
    profile: &TaxpayerProfile,
    db: Arc<Mutex<Database>>,
    transport: &T,
    authorized_form_type: fn(&str) -> Option<&'static str>,
    planner: &Planner<'_>,
) -> SendOutcome {
    let period = i64::from(summary.month.or(summary.quarter).unwrap_or(0));
    let load = |db: &Arc<Mutex<Database>>| -> Option<F> {
        let db_guard = db.lock().ok()?;
        db_guard
            .get_queueable_draft::<F>(&summary.tin, summary.taxable_year, period)
            .ok()
            .flatten()
    };
    let Some(loaded) = load(&db) else {
        return SendOutcome::NotTried;
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
                        planner.now(),
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
            return SendOutcome::NotTried;
        }
        match load(&db) {
            Some(draft) if draft.lifecycle().is_unclaimed() => draft,
            _ => return SendOutcome::NotTried,
        }
    } else {
        loaded
    };

    let Some(generation) = queue_generation(&loaded) else {
        return SendOutcome::NotTried;
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
        return SendOutcome::NotTried;
    }

    match queued_retry_is_due(draft.lifecycle().next_retry_at.as_deref(), planner.now()) {
        Ok(false) if !previous_tin_refusal(draft.lifecycle().submission_error.as_deref()) => {
            return SendOutcome::NotTried;
        }
        Ok(_) => {}
        Err(error) => {
            // Never Draft for a bad time: the tick stamps a real one.
            warn!(
                "Cron: queued {} {} has an unreadable retry time ({}); it waits for the queue to stamp a real one",
                F::FORM_CODE,
                draft.period_code(),
                error
            );
            return SendOutcome::NotTried;
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
            return SendOutcome::NotTried;
        }
    };
    let Some(form_type) = authorized_form_type(F::FORM_CODE) else {
        warn!(
            "Cron: Refusing to submit {} because {} is not authorized for queue submission",
            draft.period_code(),
            F::FORM_CODE
        );
        return SendOutcome::NotTried;
    };

    let session = match planner
        .session(transport.open_session(form_type, draft.tin()))
        .await
    {
        Ok(session) => session,
        Err(error) if smart_queue::is_final_reason(&error.0) => {
            refuse_unclaimed_queueable(
                &mut draft,
                &generation,
                &db,
                profile,
                refusal_reason(SESSION_FAILED_PREFIX, &error),
            );
            return SendOutcome::Failed(TryClass::Refused);
        }
        Err(error) => {
            let class = transport.session_failure_class(&error);
            fail_queueable(
                &mut draft,
                &generation,
                &db,
                format!("{SESSION_FAILED_PREFIX}{error}"),
                planner,
                class,
            );
            return SendOutcome::Failed(class);
        }
    };

    // Claim only after the session is open, immediately before PUT.
    let claim = {
        let Ok(db_guard) = db.lock() else {
            return SendOutcome::NotTried;
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
            return SendOutcome::NotTried;
        }
        Ok(crate::db::ClaimQueueableResult::Superseded) => {
            info!(
                "Cron: {} submission job for {} was canceled or superseded before PUT",
                F::FORM_CODE,
                draft.period_code()
            );
            return SendOutcome::NotTried;
        }
        Err(error) => {
            warn!(
                "Cron: Could not claim queued {} {} because the database claim failed: {}",
                F::FORM_CODE,
                draft.period_code(),
                error
            );
            return SendOutcome::NotTried;
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
        return SendOutcome::NotTried;
    }

    let state_before = draft.lifecycle().smart_queue.clone();
    match planner
        .within_deadline(transport.store_session_reported(session, &filename, payload.as_bytes()))
        .await
    {
        Ok(report) => {
            let dry_run = dry_run || report.dry_run;
            draft.lifecycle_mut().submission_receipt = report.receipt;
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
            SendOutcome::Sent
        }
        Err(failure) if is_final(&failure) => {
            let reason = refusal_reason(NOT_SENT_PREFIX, failure.error());
            warn!(
                "Cron: {} {} was refused for good (nothing reached BIR); back to Draft: {}",
                F::FORM_CODE,
                draft.period_code(),
                reason
            );
            if let Ok(db_guard) = db.lock() {
                match db_guard.refuse_claimed_queueable_tin::<F>(
                    draft.tin(),
                    draft.taxable_year(),
                    draft.period_column(),
                    &claim_token,
                    reason.clone(),
                ) {
                    Ok(true) => record_refusal_alert(
                        &db_guard,
                        profile,
                        F::FORM_CODE,
                        &queueable_period_label(&draft),
                        &reason,
                    ),
                    Ok(false) => info!(
                        "Cron: {} {} claim changed before its refusal was recorded",
                        F::FORM_CODE,
                        draft.period_code()
                    ),
                    Err(db_error) => warn!(
                        "Cron: could not record the refused {} send: {db_error}",
                        F::FORM_CODE
                    ),
                }
            }
            SendOutcome::Failed(TryClass::Refused)
        }
        Err(
            failure @ (StoreFailure::NotSent(_)
            | StoreFailure::NotATry { .. }
            | StoreFailure::Counted { .. }),
        ) => {
            let (ending, class) = not_sent_ending(&failure);
            let error = failure.error();
            let retry = planner.retry(&state_before, ending);
            log_not_sent(F::FORM_CODE, &draft.period_code(), error, &retry);
            if let Ok(db_guard) = db.lock()
                && let Err(db_error) = db_guard.fail_claimed_queueable_not_sent::<F>(
                    draft.tin(),
                    draft.taxable_year(),
                    draft.period_column(),
                    &claim_token,
                    format!("{NOT_SENT_PREFIX}{error}"),
                    &retry,
                )
            {
                warn!(
                    "Cron: could not record the failed {} send: {db_error}",
                    F::FORM_CODE
                );
            }
            SendOutcome::Failed(class)
        }
        Err(StoreFailure::MaybeSent(error)) => {
            warn!(
                "Cron: {} PUT ended after the network claim; outcome is unknown and support-assisted manual reconciliation is required: {}",
                F::FORM_CODE,
                error
            );
            if let Ok(db_guard) = db.lock() {
                let _ = db_guard.note_claimed_queueable_outcome_unknown::<F>(
                    draft.tin(),
                    draft.taxable_year(),
                    draft.period_column(),
                    &claim_token,
                    format!("Needs attention: {error}"),
                );
            }
            SendOutcome::Failed(TryClass::MaybeSent)
        }
    }
}

/// [`process_queued_queueable_in`] on the wall clock and the default
/// numbers, as the worker tests call it.
#[cfg(test)]
async fn process_queued_queueable<F: QueueableForm, T: SubmissionTransport>(
    summary: &FormDraftSummary,
    profile: &TaxpayerProfile,
    db: Arc<Mutex<Database>>,
    transport: &T,
    authorized_form_type: fn(&str) -> Option<&'static str>,
) {
    let env = QueueEnv::system();
    let planner = Planner::new(&env, QueuePolicy::default(), false);
    process_queued_queueable_in::<F, T>(
        summary,
        profile,
        db,
        transport,
        authorized_form_type,
        &planner,
    )
    .await;
}

/// Run the generic worker for a queueable form code. `false` when the code
/// is not a [`QueueableKind`].
async fn process_queued_by_kind<T: SubmissionTransport>(
    summary: &FormDraftSummary,
    profile: &TaxpayerProfile,
    db: Arc<Mutex<Database>>,
    transport: &T,
    planner: &Planner<'_>,
) -> SendOutcome {
    let Some(kind) = QueueableKind::from_form_code(&summary.form_code) else {
        return SendOutcome::NotTried;
    };
    crate::with_queueable_kind!(kind, F => {
        process_queued_queueable_in::<F, _>(
            summary,
            profile,
            db,
            transport,
            crate::forms::queue_authorized_form_type_id,
            planner,
        )
        .await
    })
}

#[cfg(test)]
fn submission_queue_year_at<Tz: chrono::TimeZone>(now: &chrono::DateTime<Tz>) -> u16 {
    use chrono::Datelike;
    now.year() as u16
}

/// One queued 2551Q, against the current profile of its TIN.
async fn process_queued_2551q_in<T: SubmissionTransport>(
    summary: &FormDraftSummary,
    profile: &TaxpayerProfile,
    db: Arc<Mutex<Database>>,
    transport: &T,
    planner: &Planner<'_>,
) -> SendOutcome {
    let (loaded_draft, current_profile) = {
        let db_guard = match db.lock() {
            Ok(g) => g,
            Err(_) => return SendOutcome::NotTried,
        };
        let current_profile = match db_guard.get_profile(&profile.tin.full()) {
            Ok(Some(profile)) => profile,
            _ => return SendOutcome::NotTried,
        };
        let draft = match db_guard.get_2551q_draft(
            &current_profile.tin.full(),
            summary.taxable_year,
            summary.quarter.unwrap_or(0),
        ) {
            Ok(Some(draft)) => draft,
            _ => return SendOutcome::NotTried,
        };
        (draft, current_profile)
    };

    if loaded_draft.submission_claim_token.is_some() || loaded_draft.submission_claimed_at.is_some()
    {
        let recovered = {
            let db_guard = match db.lock() {
                Ok(guard) => guard,
                Err(_) => return SendOutcome::NotTried,
            };
            db_guard
                .recover_expired_unstarted_2551q_claim(
                    &loaded_draft.tin,
                    loaded_draft.taxable_year,
                    loaded_draft.quarter,
                    planner.now(),
                )
                .unwrap_or(false)
        };
        if !recovered {
            warn!(
                "Cron: Form {} has an unresolved submission outcome; automatic retry is disabled and support-assisted manual reconciliation is required",
                loaded_draft.period_code()
            );
            return SendOutcome::NotTried;
        }
        return SendOutcome::NotTried;
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
                if let Ok(db_guard) = db.lock() {
                    let _ = db_guard.replace_unclaimed_queued_2551q_submission(
                        &draft,
                        &revision.fingerprint,
                        &revision.next_retry_at,
                        revision.submission_attempts,
                    );
                }
                return SendOutcome::NotTried;
            }
            Queued2551QPreparation::Superseded => return SendOutcome::NotTried,
        };

    match queued_retry_is_due(draft.next_retry_at.as_deref(), planner.now()) {
        Ok(false) if !previous_tin_refusal(draft.last_error.as_deref()) => {
            return SendOutcome::NotTried;
        }
        Ok(_) => {}
        Err(error) => {
            // Never Draft for a bad time: the tick stamps a real one.
            warn!(
                "Cron: queued form {} has an unreadable retry time ({}); it waits for the queue to stamp a real one",
                draft.period_code(),
                error
            );
            return SendOutcome::NotTried;
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
            if let Ok(db_guard) = db.lock() {
                let _ = db_guard.replace_unclaimed_queued_2551q_submission(
                    &draft,
                    &queue_revision.fingerprint,
                    &queue_revision.next_retry_at,
                    queue_revision.submission_attempts,
                );
            }
            return SendOutcome::NotTried;
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
        return SendOutcome::NotTried;
    };

    let session = match planner
        .session(transport.open_session(form_type, &draft.tin))
        .await
    {
        Ok(session) => session,
        Err(error) if smart_queue::is_final_reason(&error.0) => {
            refuse_unclaimed_2551q(
                &mut draft,
                &queue_revision,
                &db,
                &current_profile,
                refusal_reason(SESSION_FAILED_PREFIX, &error),
            );
            return SendOutcome::Failed(TryClass::Refused);
        }
        Err(error) => {
            let class = transport.session_failure_class(&error);
            fail_draft_2551q(
                &mut draft,
                &queue_revision,
                db.clone(),
                format!("{SESSION_FAILED_PREFIX}{error}"),
                planner,
                class,
            );
            return SendOutcome::Failed(class);
        }
    };

    // Claim only after the session is open, immediately before PUT.
    let claim_result = {
        let db_guard = match db.lock() {
            Ok(guard) => guard,
            Err(_) => return SendOutcome::NotTried,
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
            return SendOutcome::NotTried;
        }
        Ok(Claim2551QSubmissionResult::Superseded) => {
            info!(
                "Cron: Submission job for {} was canceled or superseded before PUT",
                draft.period_code()
            );
            return SendOutcome::NotTried;
        }
        Err(_) => {
            warn!(
                "Cron: Could not claim queued form {} because the database claim failed",
                draft.period_code()
            );
            return SendOutcome::NotTried;
        }
    };

    let dry_run = transport.is_dry_run();
    let put_marked = {
        let db_guard = match db.lock() {
            Ok(guard) => guard,
            Err(_) => return SendOutcome::NotTried,
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
        return SendOutcome::NotTried;
    }
    let state_before = draft.smart_queue.clone();
    match planner
        .within_deadline(transport.store_session_reported(session, &filename, xml_payload.as_bytes()))
        .await
    {
        Ok(report) => {
            let dry_run = dry_run || report.dry_run;
            draft.submission_receipt = report.receipt;
            if dry_run {
                info!(
                    "Cron: dry run: simulated submission of queued form {} (nothing sent to BIR)",
                    filename
                );
            } else {
                info!("Cron: Successfully submitted queued form {}", filename);
            }
            let period_label = quarterly_period_label(draft.taxable_year, draft.quarter);
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
            if let Ok(db_guard) = db.lock() {
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
            SendOutcome::Sent
        }
        Err(failure) if is_final(&failure) => {
            let reason = refusal_reason(NOT_SENT_PREFIX, failure.error());
            warn!(
                "Cron: Form {} was refused for good (nothing reached BIR); back to Draft: {}",
                draft.period_code(),
                reason
            );
            if let Ok(db_guard) = db.lock() {
                match db_guard.refuse_claimed_2551q_tin(
                    &draft.tin,
                    draft.taxable_year,
                    draft.quarter,
                    &claim_token,
                    reason.clone(),
                ) {
                    Ok(true) => record_refusal_alert(
                        &db_guard,
                        &current_profile,
                        "2551Q",
                        &quarterly_period_label(draft.taxable_year, draft.quarter),
                        &reason,
                    ),
                    Ok(false) => info!(
                        "Cron: Form {} claim changed before its refusal was recorded",
                        draft.period_code()
                    ),
                    Err(db_error) => {
                        warn!("Cron: could not record the refused 2551Q send: {db_error}")
                    }
                }
            }
            SendOutcome::Failed(TryClass::Refused)
        }
        Err(
            failure @ (StoreFailure::NotSent(_)
            | StoreFailure::NotATry { .. }
            | StoreFailure::Counted { .. }),
        ) => {
            let (ending, class) = not_sent_ending(&failure);
            let error = failure.error();
            let retry = planner.retry(&state_before, ending);
            log_not_sent("2551Q", &draft.period_code(), error, &retry);
            if let Ok(db_guard) = db.lock()
                && let Err(db_error) = db_guard.fail_claimed_2551q_not_sent(
                    &draft.tin,
                    draft.taxable_year,
                    draft.quarter,
                    &claim_token,
                    format!("{NOT_SENT_PREFIX}{error}"),
                    &retry,
                )
            {
                warn!("Cron: could not record the failed 2551Q send: {db_error}");
            }
            SendOutcome::Failed(class)
        }
        Err(StoreFailure::MaybeSent(error)) => {
            // Once PUT has started, an error does not prove that BIR
            // received no bytes. Keep the durable claim just like a
            // process crash: retrying could duplicate a return.
            warn!(
                %error,
                "Cron: Submission PUT ended after the network claim; outcome is unknown and support-assisted manual reconciliation is required"
            );
            SendOutcome::Failed(TryClass::MaybeSent)
        }
    }
}

/// One queued return as the tick sees it.
struct QueuedRow {
    summary: FormDraftSummary,
    profile: TaxpayerProfile,
    waiting: Waiting,
    /// Its own time, before the queue-wide back-off: what a row without
    /// queue facts keeps as its own when it is stamped behind another.
    own: Option<DateTime<Utc>>,
    /// Its stored `next_retry_at`; `None` when absent or unreadable.
    stored_next: Option<DateTime<Utc>>,
    /// The stored `next_retry_at` is there but does not parse: the tick
    /// stamps a real one (never Draft for a bad time).
    invalid_next: bool,
    /// The queue's facts on the row.
    state: QueueState,
    /// When it was queued, if the row says (its authorization, or when the
    /// queue first saw it).
    queued_at: Option<DateTime<Utc>>,
}

/// A return's form and period, for its due date.
type DueKey = (String, u16, Option<u8>, Option<u8>);

/// Every queued return of a TIN this database has a profile for, with what
/// [`smart_queue::line_up_after`] needs. Read from the stored JSON; nothing
/// is written here. Returns moved to the back of the line count only while
/// the run of path failures that moved them (`path`) lasts.
fn queued_rows(
    db: &Database,
    profiles: &[TaxpayerProfile],
    now: DateTime<Utc>,
    path: &PathBackoff,
) -> Vec<QueuedRow> {
    let by_tin: HashMap<String, &TaxpayerProfile> = profiles
        .iter()
        .map(|profile| (profile.tin.full(), profile))
        .collect();
    let path_since = path.since();
    let mut due_dates: HashMap<DueKey, Option<chrono::NaiveDate>> = HashMap::new();
    let mut rows = Vec::new();
    for summary in db.list_all_queued_submissions().unwrap_or_default() {
        if summary.status != FilingStatus::Queued
            || !crate::forms::can_queue_for_submission(&summary.form_code)
        {
            continue;
        }
        let Some(profile) = by_tin.get(&summary.tin) else {
            continue;
        };
        let Ok(Some((_, json))) = db.draft_status_and_json(summary.id) else {
            continue;
        };
        let Ok(data) = serde_json::from_str::<serde_json::Value>(&json) else {
            continue;
        };
        let text = |key: &str| {
            data.get(key)
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.is_empty())
        };
        let claimed =
            text("submission_claim_token").is_some() || text("submission_claimed_at").is_some();
        // A lease is read against the tick's clock, like every other time.
        let recoverable = claimed
            && crate::filing_queue::claim_is_recoverable(
                text("submission_claimed_at"),
                text("submission_claim_lease_until"),
                text("submission_put_started_at"),
                now,
            );
        let error = text("submission_error").or_else(|| {
            (summary.form_code == "2551Q")
                .then(|| text("last_error"))
                .flatten()
        });
        let state: QueueState = data
            .get("smart_queue")
            .and_then(|value| serde_json::from_value(value.clone()).ok())
            .unwrap_or_default();
        let stored_raw = text("next_retry_at");
        let stored_next = stored_raw.and_then(smart_queue::parse_time);
        let invalid_next = stored_raw.is_some() && stored_next.is_none();
        // Its own time, from when its last try ended; without queue facts
        // (an older build, a fresh queueing), the stored time. A row whose
        // last reason is a TIN refusal is due now: the worker decides it at
        // once. One whose stored time does not parse and has no own time is
        // due now too, and gets a real stamp.
        let own = if previous_tin_refusal(error) {
            None
        } else {
            state.ready_at(now).or(stored_next)
        };
        let queued_at = data
            .get("queue_authorization")
            .and_then(|auth| auth.get("authorized_at"))
            .and_then(serde_json::Value::as_str)
            .and_then(smart_queue::parse_time)
            .or_else(|| state.queued_at.as_deref().and_then(smart_queue::parse_time));
        let due = *due_dates
            .entry((
                summary.form_code.clone(),
                summary.taxable_year,
                summary.month,
                summary.quarter,
            ))
            .or_insert_with(|| return_due_date(&summary));
        let rotated = state
            .rotated()
            .filter(|at| path_since.is_some_and(|since| *at >= since));
        rows.push(QueuedRow {
            waiting: Waiting {
                due,
                queued_at: queued_at.unwrap_or(now),
                ready_at: own,
                blocked: (claimed && !recoverable) || state.is_stuck(),
                rotated,
                try_anyway: state.try_anyway,
            },
            own,
            profile: (*profile).clone(),
            summary,
            stored_next,
            invalid_next,
            state,
            queued_at,
        });
    }
    rows
}

/// A return's BIR due date (Manila), when the calendar knows it.
fn return_due_date(summary: &FormDraftSummary) -> Option<chrono::NaiveDate> {
    use crate::calendar_rules::DeadlinePeriod;
    let taxable_year = i32::from(summary.taxable_year);
    let period = match (summary.month, summary.quarter) {
        (Some(month), _) if (1..=12).contains(&month) => DeadlinePeriod::Monthly {
            taxable_year,
            month,
        },
        (_, Some(quarter)) if (1..=4).contains(&quarter) => DeadlinePeriod::Quarterly {
            taxable_year,
            quarter,
        },
        _ => DeadlinePeriod::Annual { taxable_year },
    };
    crate::forms::penalty_items::resolve_due_date(&summary.form_code, &period, &[])
}

/// Change an unclaimed queued row's `next_retry_at` and queue facts, only
/// when `edit` changes them, and without touching its `updated_at` (a
/// stamp is not an edit). `false` when nothing changed, or the row changed
/// meanwhile, or is no longer queued and unclaimed.
fn edit_queued_row(
    db: &Database,
    summary: &FormDraftSummary,
    edit: impl FnOnce(&mut Option<String>, &mut QueueState),
) -> bool {
    let Ok(Some((status, json))) = db.draft_status_and_json(summary.id) else {
        return false;
    };
    if status != "Queued" {
        return false;
    }
    let Ok(mut data) = serde_json::from_str::<serde_json::Value>(&json) else {
        return false;
    };
    let Some(map) = data.as_object_mut() else {
        return false;
    };
    let text = |key: &str| {
        map.get(key)
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    if text("submission_claim_token").is_some()
        || text("submission_claimed_at").is_some()
        || text("status").as_deref() != Some("Queued")
    {
        return false;
    }
    let before_next = text("next_retry_at");
    let before_state: QueueState = map
        .get("smart_queue")
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or_default();
    let (mut next, mut state) = (before_next.clone(), before_state.clone());
    edit(&mut next, &mut state);
    if next == before_next && state == before_state {
        return false;
    }
    map.insert(
        "next_retry_at".to_owned(),
        next.map_or(serde_json::Value::Null, serde_json::Value::String),
    );
    if state.is_empty() {
        map.remove("smart_queue");
    } else if let Ok(value) = serde_json::to_value(&state) {
        map.insert("smart_queue".to_owned(), value);
    }
    let Ok(new_json) = serde_json::to_string(&data) else {
        return false;
    };
    db.restamp_queued_draft(summary.id, &json, &new_json)
        .unwrap_or(false)
}

/// The period of a queued return as notices print it.
fn summary_period_label(summary: &FormDraftSummary) -> String {
    match (summary.month, summary.quarter) {
        (Some(month), _) if month > 0 => monthly_period_label(summary.taxable_year, month),
        (_, Some(quarter)) if quarter > 0 => quarterly_period_label(summary.taxable_year, quarter),
        _ => summary.taxable_year.to_string(),
    }
}

/// The "still waiting" notice's kind for one return.
fn still_waiting_kind(summary: &FormDraftSummary) -> String {
    format!(
        "{}:{}:{}",
        crate::db::alert_kinds::QUEUE_STILL_WAITING,
        summary.form_code,
        summary_period_label(summary)
    )
}

/// The words of the public contract (`docs/bir-uptime.md` in
/// `buwiz-com/buwiz-com`), once per `notice_after_s` of waiting.
fn record_still_waiting_alert(
    db: &Database,
    profile: &TaxpayerProfile,
    summary: &FormDraftSummary,
    notices: u32,
    policy: &QueuePolicy,
) {
    let hours = u64::from(policy.notice_after_s) * u64::from(notices) / 3600;
    let title = format!(
        "BIR Form {} {} is still waiting to send",
        summary.form_code,
        summary_period_label(summary)
    );
    let detail = format!(
        "{}\nTIN {}\nStill waiting after {hours} hours. It keeps trying by itself; you can cancel it or file another way.",
        profile.full_name,
        profile.tin.formatted()
    );
    let _ = db.record_alert(
        Some(&profile.tin.full()),
        &still_waiting_kind(summary),
        crate::db::AlertSeverity::Warning,
        &title,
        &detail,
        crate::db::AlertAction::None,
    );
}

/// The settings key of the Manila day the paused reminder was last raised.
const PAUSED_REMINDER_SETTING: &str = "smart_queue_paused_reminded";

/// While the queue is paused with returns waiting, one reminder a day.
fn remind_paused(db: &Database, profiles: &[TaxpayerProfile], now: DateTime<Utc>) {
    let waiting = queued_rows(db, profiles, now, &PathBackoff::default())
        .iter()
        .filter(|row| !row.waiting.blocked)
        .count();
    if waiting == 0 {
        return;
    }
    let today = (now + chrono::Duration::hours(8)).date_naive().to_string();
    if db
        .get_setting(PAUSED_REMINDER_SETTING)
        .ok()
        .flatten()
        .as_deref()
        == Some(today.as_str())
    {
        return;
    }
    let returns = if waiting == 1 {
        "1 return is".to_owned()
    } else {
        format!("{waiting} returns are")
    };
    let _ = db.record_alert(
        None,
        crate::db::alert_kinds::QUEUE_PAUSED,
        crate::db::AlertSeverity::Info,
        "Your queue is paused",
        &format!("Your queue is paused · press Auto to send. {returns} waiting; nothing goes to the BIR until then."),
        crate::db::AlertAction::None,
    );
    let _ = db.set_setting(PAUSED_REMINDER_SETTING, &today);
}

/// The databases a tick runs on in this process right now: a second tick on
/// the same database returns at once, so two sends never overlap. (Two
/// processes are kept apart by the worker lease.)
fn ticking() -> &'static Mutex<HashSet<usize>> {
    static TICKING: OnceLock<Mutex<HashSet<usize>>> = OnceLock::new();
    TICKING.get_or_init(|| Mutex::new(HashSet::new()))
}

struct TickGuard(usize);

impl TickGuard {
    fn take(db: &Arc<Mutex<Database>>) -> Option<Self> {
        let key = Arc::as_ptr(db) as usize;
        let mut running = ticking().lock().ok()?;
        // Not `then_some`: a guard built for a refused take would drop, and
        // unlock someone else's tick, while this lock is held.
        if running.insert(key) {
            Some(Self(key))
        } else {
            None
        }
    }
}

impl Drop for TickGuard {
    fn drop(&mut self) {
        if let Ok(mut running) = ticking().lock() {
            running.remove(&self.0);
        }
    }
}

/// Order the queue and stamp every waiting row whose stored time changes:
/// the head now, every other row its own time but a minute apart and never
/// before the queue-wide back-off. A row without queue facts keeps its own
/// time when it is stamped later. Answers the line.
fn stamp_line(
    db: &Database,
    rows: &[QueuedRow],
    now: DateTime<Utc>,
    policy: &QueuePolicy,
    path: &PathBackoff,
    head_now: bool,
) -> smart_queue::LineUp {
    let waiting: Vec<Waiting> = rows.iter().map(|row| row.waiting.clone()).collect();
    // After a send nobody is stamped "now": the next return goes when the
    // worker wakes for it, and an older build sharing the database finds
    // every row a minute or more ahead.
    let basis = if head_now {
        now
    } else {
        now + chrono::Duration::seconds(smart_queue::BEHIND_GAP_S)
    };
    let line = smart_queue::line_up_after(&waiting, basis, policy, path.floor(now));
    for (i, row) in rows.iter().enumerate() {
        let Some(stamp) = line.stamps[i] else {
            continue;
        };
        let head = head_now && Some(i) == line.head;
        let changes = row.invalid_next
            || match row.stored_next {
                None => true,
                // The head goes now: a time stored later is brought forward
                // so the worker finds it due.
                Some(stored) if head => stored > now,
                Some(stored) => (stored - stamp).num_seconds().abs() >= 1,
            };
        if !changes {
            continue;
        }
        let own = row.own;
        let at = if head { now } else { stamp };
        edit_queued_row(db, &row.summary, |next, state| {
            *next = Some(at.to_rfc3339());
            if state.next_try_at.is_none()
                && !head
                && let Some(own) = own
            {
                state.next_try_at = Some(own.to_rfc3339());
            }
        });
    }
    line
}

/// The settings key that remembers how many "still waiting" notices one
/// queueing of a return raised, so an older build that rewrote the row
/// (and dropped the queue's facts) does not make it raise them again.
fn notice_setting_key(row: &QueuedRow) -> String {
    format!(
        "smart_queue_notice:{}:{}",
        row.profile.tin.full(),
        still_waiting_kind(&row.summary)
    )
}

/// One "still waiting" notice per `notice_after_s` of waiting since the
/// return was queued (its authorization, never when its draft was made),
/// once each, for the rows still waiting by themselves.
fn notice_rows(db: &Database, rows: &[QueuedRow], now: DateTime<Utc>, policy: &QueuePolicy) {
    for row in rows.iter().filter(|row| !row.waiting.blocked) {
        let Some(queued_at) = row.queued_at else {
            // No time on the row: the queue's first sight is when it began.
            edit_queued_row(db, &row.summary, |_, state| {
                state.queued_at = Some(now.to_rfc3339());
            });
            continue;
        };
        let due = smart_queue::notices_due(queued_at, now, policy);
        if due == 0 {
            continue;
        }
        let key = notice_setting_key(row);
        let stamp = queued_at.to_rfc3339();
        let remembered = db
            .get_setting(&key)
            .ok()
            .flatten()
            .and_then(|stored| {
                let (at, count) = stored.split_once(' ')?;
                (at == stamp).then(|| count.parse::<u32>().ok()).flatten()
            })
            .unwrap_or(0);
        if due > row.state.notices.max(remembered) {
            record_still_waiting_alert(db, &row.profile, &row.summary, due, policy);
            let _ = db.set_setting(&key, &format!("{stamp} {due}"));
        }
        if due > row.state.notices {
            edit_queued_row(db, &row.summary, |_, state| state.notices = due);
        }
    }
}

/// A "still waiting" notice stays only while its return still waits by
/// itself: cancelled, refused, stopped, needing attention or sent, it goes.
fn clear_stale_notices(db: &Database, rows: &[QueuedRow]) {
    let waiting: HashSet<(String, String)> = rows
        .iter()
        .filter(|row| !row.waiting.blocked)
        .map(|row| (row.profile.tin.full(), still_waiting_kind(&row.summary)))
        .collect();
    let prefix = format!("{}:", crate::db::alert_kinds::QUEUE_STILL_WAITING);
    for alert in db.list_active_alerts(None).unwrap_or_default() {
        let tin = alert.tin.clone().unwrap_or_default();
        if alert.kind.starts_with(&prefix) && !waiting.contains(&(tin, alert.kind.clone())) {
            let _ = db.resolve_alert(alert.tin.as_deref(), &alert.kind);
        }
    }
}

/// Send one row through its form's worker.
async fn send_row<T: SubmissionTransport>(
    row: &QueuedRow,
    db: &Arc<Mutex<Database>>,
    transport: &T,
    planner: &Planner<'_>,
) -> SendOutcome {
    match row.summary.form_code.as_str() {
        "2551Q" => {
            process_queued_2551q_in(&row.summary, &row.profile, db.clone(), transport, planner)
                .await
        }
        "1601C" => {
            process_queued_1601c_in(&row.summary, &row.profile, db.clone(), transport, planner)
                .await
        }
        _ => process_queued_by_kind(&row.summary, &row.profile, db.clone(), transport, planner).await,
    }
}

/// A queue tick on `env`'s clock and random source: at most one return is
/// sent, the head of the line across every TIN, and never before the
/// queue-wide back-off; every other queued return keeps a real next try
/// ([`smart_queue::line_up_after`]). A head the worker could not take
/// (not tried) gets a growing wait of its own and the next head is tried,
/// so one bad row never starves the queue. Returns how the send ended, or
/// `None` when nothing was due, the queue is paused or another tick on
/// this database is running.
pub async fn run_queue_tick_with<T: SubmissionTransport + 'static>(
    db: Arc<Mutex<Database>>,
    transport: &Arc<T>,
    env: &QueueEnv,
) -> Option<SendOutcome> {
    let _ticking = TickGuard::take(&db)?;
    let now = env.now();
    let (profiles, global_cron_enabled, mode, tries, mut path) = {
        let db_guard = match db.lock() {
            Ok(guard) => guard,
            Err(e) => {
                error!("Failed to lock database for cron: {}", e);
                return None;
            }
        };
        let p = match db_guard.list_profiles() {
            Ok(p) => p,
            Err(e) => {
                error!("Failed to fetch profiles in cron: {}", e);
                return None;
            }
        };
        let c = db_guard
            .get_setting("background_cron_enabled")
            .unwrap_or(Some("true".to_string()))
            .map(|s| s == "true")
            .unwrap_or(true);
        let setting = |key: &str| db_guard.get_setting(key).ok().flatten();
        let mode = QueueMode::from_setting(setting(smart_queue::QUEUE_MODE_SETTING).as_deref());
        let tries = setting(smart_queue::DEVICE_TRIES_SETTING)
            .map(|stored| DeviceTries::from_setting(&stored))
            .unwrap_or_default();
        let path = PathBackoff::from_setting(setting(smart_queue::PATH_SETTING).as_deref());
        (p, c, mode, tries, path)
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

    // The person's Pause switch stops everything, next to the hidden one.
    if !global_cron_enabled {
        return None;
    }
    if mode == QueueMode::Paused {
        if let Ok(db_guard) = db.lock() {
            remind_paused(&db_guard, &profiles, now);
        }
        return None;
    }

    // Without a sign (every server today) the defaults; a sign's numbers
    // are already clamped so they can only lengthen the waits.
    let sign = transport.sign();
    let policy = sign
        .as_ref()
        .and_then(|seen| seen.sign.queue.clone())
        .unwrap_or_default();
    let sign_state = sign
        .as_ref()
        .filter(|seen| seen.is_fresh(now, &policy))
        .map(|seen| seen.sign.state);
    let path_before = path.clone();

    // The head; one the worker could not take steps aside on a wait of its
    // own, and the next is tried. Each row at most once a tick.
    let mut seen_heads: HashSet<i64> = HashSet::new();
    let mut last: Option<(SendOutcome, Option<Blame>, QueuedRow)> = None;
    loop {
        let (rows, head) = {
            let Ok(db_guard) = db.lock() else {
                return None;
            };
            // A clock that went back (it reads earlier than when a try
            // ended): the waits owed are owed from now, stored once.
            if path.reanchor(now) {
                let _ = db_guard.set_setting(smart_queue::PATH_SETTING, &path.to_setting());
            }
            let mut rows = queued_rows(&db_guard, &profiles, now, &path);
            let mut moved = false;
            for row in &rows {
                if row.state.ended_at.as_deref().and_then(smart_queue::parse_time).is_some_and(|ended| ended > now) {
                    moved |= edit_queued_row(&db_guard, &row.summary, |next, state| {
                        state.reanchor(now, next);
                    });
                }
            }
            if moved {
                rows = queued_rows(&db_guard, &profiles, now, &path);
            }
            let line = stamp_line(&db_guard, &rows, now, &policy, &path, true);
            if seen_heads.is_empty() {
                notice_rows(&db_guard, &rows, now, &policy);
                clear_stale_notices(&db_guard, &rows);
            }
            (rows, line.head)
        };
        let Some(row) = head.and_then(|head| rows.into_iter().nth(head)) else {
            break;
        };
        if !seen_heads.insert(row.summary.id) {
            break;
        }
        let job_key = queue_job_key(&row.summary);
        let active_jobs = get_active_jobs();
        {
            let mut jobs = active_jobs.lock().ok()?;
            if !jobs.insert(job_key.clone()) {
                return None; // Already running
            }
        }
        let cleanup = JobCleanup {
            key: job_key,
            active_jobs: active_jobs.clone(),
        };
        let planner = Planner::with_path(
            env,
            policy.clone(),
            smart_queue::guard_trips(sign_state, &tries),
            path.clone(),
        );
        let outcome = send_row(&row, &db, &**transport, &planner).await;
        drop(cleanup);
        path = planner.path();
        if outcome == SendOutcome::NotTried {
            if let Ok(db_guard) = db.lock() {
                env.with_rng(|rng| {
                    edit_queued_row(&db_guard, &row.summary, |next, state| {
                        state.skipped(now, &policy, rng);
                        next.clone_from(&state.next_try_at);
                    })
                });
            }
            info!(
                "Cron: {} {} was not tried; it waits a while, and the next return goes",
                row.summary.form_code,
                summary_period_label(&row.summary)
            );
            last = Some((outcome, None, row));
            continue;
        }
        last = Some((outcome, planner.last_blame(), row));
        break;
    }
    let (outcome, blame, row) = last?;

    // This device's tries toward the BIR, for the guard: a landing, or a
    // failure that says something about its way there. Not a try, a
    // refusal, an unknown outcome and a failure about the return itself
    // (a refused login, no upload service, a validation refusal, Buwiz's
    // own errors) say nothing about this device's connection.
    let mut after = tries.clone();
    match (outcome, blame) {
        (SendOutcome::Sent, _) => after.record(true),
        (SendOutcome::Failed(_), Some(blame)) if blame.heard_by_guard() => after.record(false),
        _ => {}
    }
    // A landing anywhere: the way in works again.
    if outcome == SendOutcome::Sent {
        path.clear();
    }
    if let Ok(db_guard) = db.lock() {
        if after != tries {
            let _ = db_guard.set_setting(smart_queue::DEVICE_TRIES_SETTING, &after.to_setting());
        }
        if path != path_before {
            let _ = db_guard.set_setting(smart_queue::PATH_SETTING, &path.to_setting());
        }
        if matches!(
            outcome,
            SendOutcome::Sent | SendOutcome::Failed(TryClass::Refused | TryClass::MaybeSent)
        ) {
            let _ = db_guard.resolve_alert(
                Some(&row.profile.tin.full()),
                &still_waiting_kind(&row.summary),
            );
        }
        // Stamp the line again: the return just tried keeps a minute from
        // the rows behind it, and none is before a new back-off.
        let now = env.now();
        let rows = queued_rows(&db_guard, &profiles, now, &path);
        stamp_line(&db_guard, &rows, now, &policy, &path, false);
    }
    if outcome == SendOutcome::Sent {
        // The next return goes at once, not at the next minute.
        let _ = wake_with_status();
    }
    Some(outcome)
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
        profile.tracking_mailbox().unwrap_or("")
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

/// A return Buwiz refused for its TIN gets an entry on the Notifications
/// page too, once per return period: back to Draft without a word would
/// read as lost. The text is fixed, never the server's message.
fn record_refusal_alert(
    db: &Database,
    profile: &TaxpayerProfile,
    form_code: &str,
    period_label: &str,
    reason: &str,
) {
    let kind = format!("form_refused:{form_code}:{period_label}");
    let title = format!("BIR Form {form_code} {period_label} was not sent");
    let why = if smart_queue::is_mismatch_reason(reason) {
        "Buwiz found that the return it got did not match what was checked before sending"
    } else if TinRefusal::from_reason(reason) == Some(TinRefusal::AccountBanned) {
        "Buwiz would not send it because this Buwiz account is banned"
    } else {
        "Buwiz would not send it for this TIN"
    };
    let detail = format!(
        "{}\nTIN {}\n{why}, so it is back to a draft. Nothing reached BIR. Open the return to see what to do.",
        profile.full_name,
        profile.tin.formatted()
    );
    let _ = db.record_alert(
        Some(&profile.tin.full()),
        &kind,
        crate::db::AlertSeverity::Warning,
        &title,
        &detail,
        crate::db::AlertAction::None,
    );
}

/// The session opened with a TIN refusal (a banned account): no claim was
/// taken, so the unclaimed row goes back to Draft through the same CAS a
/// failed session uses.
fn refuse_unclaimed_1601c(
    draft: &mut Form1601CDraft,
    revision: &Queued1601CRevision,
    db: &Arc<Mutex<Database>>,
    profile: &TaxpayerProfile,
    error_message: String,
) {
    warn!(
        "Cron: 1601C {} was refused for its TIN before upload; back to Draft: {}",
        draft.period_code(),
        error_message
    );
    let reason = error_message.clone();
    draft.record_tin_refusal(error_message);
    if let Ok(db_guard) = db.lock() {
        match db_guard.replace_unclaimed_queued_1601c_submission(
            draft,
            &revision.fingerprint,
            &revision.next_retry_at,
            revision.submission_attempts,
        ) {
            Ok(true) => record_refusal_alert(
                &db_guard,
                profile,
                "1601C",
                &monthly_period_label(draft.taxable_year, draft.month),
                &reason,
            ),
            Ok(false) => info!(
                "Cron: 1601C {} was canceled or superseded before its refusal persisted",
                draft.period_code()
            ),
            Err(error) => warn!(
                "Cron: Failed to persist the 1601C refusal for {}: {}",
                draft.period_code(),
                error
            ),
        }
    }
}

/// [`refuse_unclaimed_1601c`] for a 2551Q.
fn refuse_unclaimed_2551q(
    draft: &mut Form2551QDraft,
    revision: &Queued2551QRevision,
    db: &Arc<Mutex<Database>>,
    profile: &TaxpayerProfile,
    error_message: String,
) {
    warn!(
        "Cron: Form {} was refused for its TIN before upload; back to Draft: {}",
        draft.period_code(),
        error_message
    );
    let reason = error_message.clone();
    draft.record_tin_refusal(error_message);
    if let Ok(db_guard) = db.lock() {
        match db_guard.replace_unclaimed_queued_2551q_submission(
            draft,
            &revision.fingerprint,
            &revision.next_retry_at,
            revision.submission_attempts,
        ) {
            Ok(true) => record_refusal_alert(
                &db_guard,
                profile,
                "2551Q",
                &quarterly_period_label(draft.taxable_year, draft.quarter),
                &reason,
            ),
            Ok(false) => info!(
                "Cron: Form {} was canceled or superseded before its refusal persisted",
                draft.period_code()
            ),
            Err(error) => warn!(
                "Cron: Failed to persist the refusal for {}: {}",
                draft.period_code(),
                error
            ),
        }
    }
}

/// [`refuse_unclaimed_1601c`] for a form on the generic queue.
fn refuse_unclaimed_queueable<F: QueueableForm>(
    draft: &mut F,
    generation: &QueueGeneration,
    db: &Arc<Mutex<Database>>,
    profile: &TaxpayerProfile,
    error_message: String,
) {
    warn!(
        "Cron: {} {} was refused for its TIN before upload; back to Draft: {}",
        F::FORM_CODE,
        draft.period_code(),
        error_message
    );
    let reason = error_message.clone();
    draft.lifecycle_mut().record_tin_refusal(error_message);
    if let Ok(db_guard) = db.lock() {
        match db_guard.replace_unclaimed_queued_queueable(
            draft,
            &generation.fingerprint,
            &generation.next_retry_at,
            generation.submission_attempts,
        ) {
            Ok(true) => record_refusal_alert(
                &db_guard,
                profile,
                F::FORM_CODE,
                &queueable_period_label(draft),
                &reason,
            ),
            Ok(false) => info!(
                "Cron: {} {} was canceled or superseded before its refusal persisted",
                F::FORM_CODE,
                draft.period_code()
            ),
            Err(error) => warn!(
                "Cron: Failed to persist the {} refusal for {}: {}",
                F::FORM_CODE,
                draft.period_code(),
                error
            ),
        }
    }
}

fn fail_draft_2551q(
    draft: &mut crate::forms::form_2551q::Form2551QDraft,
    revision: &Queued2551QRevision,
    db: Arc<Mutex<Database>>,
    error_msg: String,
    planner: &Planner<'_>,
    class: TryClass,
) {
    let retry = planner.retry(&draft.smart_queue, session_ending(class, &error_msg));
    warn!(
        "Cron: Submission failed for {}{}: {}; it stays queued, next try at {}",
        draft.period_code(),
        if retry.was_a_try() { "" } else { " (not a try)" },
        error_msg,
        retry.at.to_rfc3339()
    );
    draft.record_failed_try(error_msg, &retry);

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
    planner: &Planner<'_>,
    class: TryClass,
) {
    let retry = planner.retry(&draft.smart_queue, session_ending(class, &error_message));
    warn!(
        "Cron: 1601C submission preparation failed for {}{}: {}; next try at {}",
        draft.period_code(),
        if retry.was_a_try() { "" } else { " (not a try)" },
        error_message,
        retry.at.to_rfc3339()
    );
    draft.record_failed_try(error_message, &retry);
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
        /// The PUT is attempted but the far side says nothing reached BIR
        /// (for example the ticket request was refused).
        NotSentAtStore,
        /// [`Self::NotSentAtStore`] with this reason, as the server worded it.
        NotSentWith(&'static str),
        /// The PUT ends with BIR maybe having the return, with this reason.
        MaybeSentWith(&'static str),
        /// The session fails before any PUT, with this reason.
        SessionFailsWith(&'static str),
    }

    struct RecordingSubmissionTransport {
        calls: Mutex<Vec<RecordedSubmission>>,
        outcome: TestTransportOutcome,
        dry_run: bool,
        /// What a successful PUT reports back; `None` keeps the trait default.
        report: Option<StoreReport>,
    }

    impl RecordingSubmissionTransport {
        fn new(outcome: TestTransportOutcome) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                outcome,
                dry_run: false,
                report: None,
            }
        }

        fn dry_run(outcome: TestTransportOutcome) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                outcome,
                dry_run: true,
                report: None,
            }
        }

        /// A transport that is not itself in dry-run mode but whose far side
        /// answers every PUT with `report` (a local server's signed receipt).
        fn reporting(report: StoreReport) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                outcome: TestTransportOutcome::Success,
                dry_run: false,
                report: Some(report),
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
                    TestTransportOutcome::SessionFailsWith(reason) => {
                        Err(TransportError(reason.to_string()))
                    }
                    TestTransportOutcome::Success
                    | TestTransportOutcome::UnknownIo
                    | TestTransportOutcome::NotSentAtStore
                    | TestTransportOutcome::NotSentWith(_)
                    | TestTransportOutcome::MaybeSentWith(_) => Ok(form_type),
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
                    TestTransportOutcome::NotSentAtStore => Err(TransportError(
                        "simulated ticket refused before upload".to_string(),
                    )),
                    TestTransportOutcome::NotSentWith(reason)
                    | TestTransportOutcome::MaybeSentWith(reason) => {
                        Err(TransportError(reason.to_string()))
                    }
                    TestTransportOutcome::PreStore | TestTransportOutcome::SessionFailsWith(_) => {
                        panic!("PUT must not run after a pre-store session failure")
                    }
                }
            })
        }

        fn store_session_reported<'a>(
            &'a self,
            session: Self::Session,
            filename: &'a str,
            payload: &'a [u8],
        ) -> Pin<Box<dyn Future<Output = Result<StoreReport, StoreFailure>> + Send + 'a>> {
            let report = self.report.clone();
            let outcome = self.outcome;
            Box::pin(async move {
                self.store_session(session, filename, payload)
                    .await
                    .map(|()| report.unwrap_or_default())
                    .map_err(|error| match outcome {
                        TestTransportOutcome::NotSentAtStore
                        | TestTransportOutcome::NotSentWith(_) => StoreFailure::NotSent(error),
                        _ => StoreFailure::MaybeSent(error),
                    })
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
        profile.email = "receipts@example.com".to_string();
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
        assert_eq!(submitted.status, FilingStatus::Sent);
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
        profile.email = "receipts@example.com".to_string();
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
        assert_eq!(submitted.status, FilingStatus::Sent);
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
    async fn process_queued_1601c_keeps_the_signed_dry_run_receipt() {
        let profile = test_profile();
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
        // The client transport is live; only the server's receipt says the
        // PUT was simulated.
        let receipt = "header.dry-run-claims.signature".to_string();
        let transport = RecordingSubmissionTransport::reporting(StoreReport {
            receipt: Some(receipt.clone()),
            dry_run: true,
        });

        process_queued_1601c_with_transport(&summary, &profile, db.clone(), &transport).await;

        let calls = transport.calls();
        assert_eq!(calls.len(), 1);
        assert_exact_1601c_transport_call(&calls[0], &queued);
        let db_guard = db.lock().expect("the cron database should not be poisoned");
        let submitted = db_guard
            .get_1601c_draft(&queued.tin, queued.taxable_year, queued.month)
            .expect("the submitted 1601C lookup should succeed")
            .expect("the submitted 1601C snapshot should remain persisted");
        assert_eq!(submitted.status, FilingStatus::Sent);
        assert_eq!(submitted.submission_receipt.as_deref(), Some(receipt.as_str()));
        assert!(submitted.submission_claim_token.is_none());
        let alerts = db_guard
            .list_active_alerts(Some(&profile.tin.full()))
            .expect("the alert lookup should succeed");
        let sent = alerts
            .iter()
            .find(|alert| alert.kind.starts_with("form_submitted:1601C:"))
            .expect("the send should record a submission alert");
        assert!(sent.title.ends_with(DRY_RUN_NOTICE_SUFFIX), "{}", sent.title);
        assert!(sent.detail.contains(DRY_RUN_NOTICE_SUFFIX.trim_start()));
    }

    #[tokio::test]
    async fn queued_2551q_keeps_the_signed_dry_run_receipt() {
        let db = Arc::new(Mutex::new(
            Database::open_in_memory_for_tests()
                .expect("the in-memory cron database should initialize"),
        ));
        let profile = db
            .lock()
            .expect("the cron database should not be poisoned")
            .save_profile(reviewed_profile())
            .expect("the reviewed profile should persist");
        let queued = queued_draft(&profile);
        db.lock()
            .expect("the cron database should not be poisoned")
            .save_queued_2551q_draft_and_election(&queued)
            .expect("the reviewed 2551Q queue snapshot should persist");
        // The client transport is live; only the server's receipt says the
        // PUT was simulated.
        let receipt = "header.dry-run-claims.signature".to_string();
        let transport = Arc::new(RecordingSubmissionTransport::reporting(StoreReport {
            receipt: Some(receipt.clone()),
            dry_run: true,
        }));

        run_queue_tick(db.clone(), &transport).await;
        // The 2551Q worker runs on a spawned task: wait for it to finish.
        let stored = || {
            db.lock()
                .expect("the cron database should not be poisoned")
                .get_2551q_draft(&queued.tin, queued.taxable_year, queued.quarter)
                .expect("the 2551Q lookup should succeed")
                .expect("the 2551Q snapshot should remain persisted")
        };
        for _ in 0..500 {
            if stored().status != FilingStatus::Queued {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        let calls = transport.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].filename, queued.default_submission_filename());
        let submitted = stored();
        assert_eq!(submitted.status, FilingStatus::Sent);
        assert_eq!(submitted.submission_receipt.as_deref(), Some(receipt.as_str()));
        assert!(submitted.submission_claim_token.is_none());
        assert!(submitted.submission_claimed_at.is_none());
        let alerts = db
            .lock()
            .expect("the cron database should not be poisoned")
            .list_active_alerts(Some(&profile.tin.full()))
            .expect("the alert lookup should succeed");
        let sent = alerts
            .iter()
            .find(|alert| alert.kind.starts_with("form_submitted:2551Q:"))
            .expect("the send should record a submission alert");
        assert!(sent.title.ends_with(DRY_RUN_NOTICE_SUFFIX), "{}", sent.title);
        assert!(sent.detail.contains(DRY_RUN_NOTICE_SUFFIX.trim_start()));
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
        assert_eq!(submitted.lifecycle.status, FilingStatus::Sent);
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
        assert_eq!(submitted.status, FilingStatus::Sent);
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
        assert_eq!(submitted.status, FilingStatus::Sent);
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
        profile.email = "taxpayer@example.com".to_string();
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
        assert_eq!(submitted.status, FilingStatus::Sent);
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

    // ----- A store failure that reached nothing at BIR -----

    fn retry_is_in_the_future(next_retry_at: Option<&str>) -> bool {
        next_retry_at
            .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
            .is_some_and(|at| at.with_timezone(&Utc) > Utc::now())
    }

    /// The `status` column of the row, which list queries read, as opposed
    /// to the status inside `data_json`.
    fn form_drafts_status_column(
        db: &Arc<Mutex<Database>>,
        form_code: &str,
        tin: &str,
        taxable_year: u16,
        slot: u8,
    ) -> String {
        db.lock()
            .expect("the cron database should not be poisoned")
            .conn
            .query_row(
                "SELECT status FROM form_drafts
                 WHERE tin = ?1 AND form_code = ?2 AND taxable_year = ?3 AND quarter = ?4",
                rusqlite::params![tin, form_code, i64::from(taxable_year), i64::from(slot)],
                |row| row.get::<_, String>(0),
            )
            .expect("the form_drafts row should exist")
    }

    fn stored_1601c(db: &Arc<Mutex<Database>>, queued: &Form1601CDraft) -> Form1601CDraft {
        db.lock()
            .expect("the cron database should not be poisoned")
            .get_1601c_draft(&queued.tin, queued.taxable_year, queued.month)
            .expect("the 1601C lookup should succeed")
            .expect("the 1601C snapshot should remain persisted")
    }

    /// Move the stored generation's retry time into the past, the way the
    /// pre-store failure test does, so the next worker run is due.
    fn make_1601c_retry_due(db: &Arc<Mutex<Database>>, queued: &Form1601CDraft) {
        let stored = stored_1601c(db, queued);
        let mut due = stored.clone();
        due.next_retry_at = Some("1999-01-01T00:00:00Z".to_string());
        assert!(
            db.lock()
                .expect("the cron database should not be poisoned")
                .replace_unclaimed_queued_1601c_submission(
                    &due,
                    &stored.queued_submission_fingerprint,
                    &stored.next_retry_at,
                    stored.submission_attempts,
                )
                .expect("forcing the retry timestamp should CAS-succeed")
        );
    }

    fn saved_queued_1601c() -> (
        TaxpayerProfile,
        Form1601CDraft,
        Arc<Mutex<Database>>,
        FormDraftSummary,
    ) {
        let profile = test_profile();
        let queued = queued_1601c_draft(&profile);
        let db = test_db();
        let id = db
            .lock()
            .expect("the cron database should not be poisoned")
            .save_queued_1601c_draft(&queued)
            .expect("the reviewed 1601C queue snapshot should persist");
        let summary = queued_1601c_summary(id, &queued);
        (profile, queued, db, summary)
    }

    #[tokio::test]
    async fn process_queued_1601c_not_sent_at_store_releases_the_claim_and_backs_off() {
        let (profile, queued, db, summary) = saved_queued_1601c();
        let transport = RecordingSubmissionTransport::new(TestTransportOutcome::NotSentAtStore);

        process_queued_1601c_with_transport(&summary, &profile, db.clone(), &transport).await;

        assert_eq!(transport.calls().len(), 1);
        let failed = stored_1601c(&db, &queued);
        assert_eq!(failed.status, FilingStatus::Queued);
        assert_eq!(
            failed.queued_submission_fingerprint,
            queued.queued_submission_fingerprint
        );
        assert_eq!(failed.submission_attempts, 1);
        assert!(failed.submission_claim_token.is_none());
        assert!(failed.submission_claimed_at.is_none());
        assert!(failed.submission_claim_lease_until.is_none());
        assert!(failed.submission_put_started_at.is_none());
        let error = failed.submission_error.as_deref().unwrap_or_default();
        assert!(error.starts_with(NOT_SENT_PREFIX), "{error}");
        assert!(
            retry_is_in_the_future(failed.next_retry_at.as_deref()),
            "{:?}",
            failed.next_retry_at
        );
        assert_eq!(
            form_drafts_status_column(&db, "1601C", &queued.tin, queued.taxable_year, queued.month),
            "Queued"
        );

        // Unclaimed again, so the person can cancel it.
        let canceled = db
            .lock()
            .expect("the cron database should not be poisoned")
            .cancel_queued_1601c_submission(&failed)
            .expect("a released not-sent row should cancel");
        assert_eq!(canceled.status, FilingStatus::Draft);
        assert_eq!(stored_1601c(&db, &queued).status, FilingStatus::Draft);
    }

    #[tokio::test]
    async fn process_queued_1601c_not_sent_at_store_retries_once_the_backoff_passes() {
        let (profile, queued, db, summary) = saved_queued_1601c();
        let failing = RecordingSubmissionTransport::new(TestTransportOutcome::NotSentAtStore);
        process_queued_1601c_with_transport(&summary, &profile, db.clone(), &failing).await;
        assert_eq!(failing.calls().len(), 1);

        // Still inside the backoff: no second PUT.
        let early = RecordingSubmissionTransport::new(TestTransportOutcome::Success);
        process_queued_1601c_with_transport(&summary, &profile, db.clone(), &early).await;
        assert!(early.calls().is_empty());
        assert_eq!(stored_1601c(&db, &queued).status, FilingStatus::Queued);

        make_1601c_retry_due(&db, &queued);
        let retry = RecordingSubmissionTransport::new(TestTransportOutcome::Success);
        process_queued_1601c_with_transport(&summary, &profile, db.clone(), &retry).await;
        let calls = retry.calls();
        assert_eq!(calls.len(), 1);
        assert_exact_1601c_transport_call(&calls[0], &queued);
        let submitted = stored_1601c(&db, &queued);
        assert_eq!(submitted.status, FilingStatus::Sent);
        assert!(submitted.submission_claim_token.is_none());
    }

    #[tokio::test]
    async fn process_queued_1601c_stays_queued_and_authorized_after_any_number_of_failures() {
        let (profile, queued, db, summary) = saved_queued_1601c();
        for attempt in 1..=7 {
            let transport = RecordingSubmissionTransport::new(TestTransportOutcome::NotSentAtStore);
            process_queued_1601c_with_transport(&summary, &profile, db.clone(), &transport).await;
            assert_eq!(transport.calls().len(), 1, "attempt {attempt}");
            assert_eq!(stored_1601c(&db, &queued).submission_attempts, attempt);
            assert_eq!(stored_1601c(&db, &queued).smart_queue.failures, attempt);
            make_1601c_retry_due(&db, &queued);
        }

        let waiting = stored_1601c(&db, &queued);
        assert_eq!(waiting.status, FilingStatus::Queued);
        assert_eq!(
            form_drafts_status_column(&db, "1601C", &queued.tin, queued.taxable_year, queued.month),
            "Queued"
        );
        assert_eq!(
            waiting.queued_submission_fingerprint,
            queued.queued_submission_fingerprint
        );
        assert_eq!(waiting.queue_authorization, queued.queue_authorization);
        assert!(waiting.submitted_payload.is_some());
        assert!(waiting.submission_claim_token.is_none());
        assert!(
            waiting
                .submission_error
                .as_deref()
                .is_some_and(|error| error.starts_with(NOT_SENT_PREFIX))
        );

        // The BIR is back: the eighth try sends it.
        let after = RecordingSubmissionTransport::new(TestTransportOutcome::Success);
        process_queued_1601c_with_transport(&summary, &profile, db.clone(), &after).await;
        assert_eq!(after.calls().len(), 1);
        assert_eq!(stored_1601c(&db, &queued).status, FilingStatus::Sent);
    }

    /// A reviewed profile under its own TIN: the queue's job key is global,
    /// so each 2551Q tick test needs a key no other test uses.
    fn reviewed_profile_with_tin(segment3: &str) -> TaxpayerProfile {
        let mut profile = test_profile();
        profile.tin.segment1 = "000".to_string();
        profile.tin.segment3 = segment3.to_string();
        profile.tax_elections.push(TaxElectionHistory {
            taxable_year: 2024,
            election: IncomeTaxElection::GraduatedUnspecified,
            elected_at: chrono::NaiveDateTime::default(),
            source_form: "2551Qv2018".to_string(),
        });
        profile.ensure_profile_version_ledger();
        profile
    }

    fn saved_queued_2551q(segment3: &str) -> (Form2551QDraft, Arc<Mutex<Database>>) {
        let db = test_db();
        let profile = db
            .lock()
            .expect("the cron database should not be poisoned")
            .save_profile(reviewed_profile_with_tin(segment3))
            .expect("the reviewed profile should persist");
        let queued = queued_draft(&profile);
        db.lock()
            .expect("the cron database should not be poisoned")
            .save_queued_2551q_draft_and_election(&queued)
            .expect("the reviewed 2551Q queue snapshot should persist");
        (queued, db)
    }

    fn stored_2551q(db: &Arc<Mutex<Database>>, queued: &Form2551QDraft) -> Form2551QDraft {
        db.lock()
            .expect("the cron database should not be poisoned")
            .get_2551q_draft(&queued.tin, queued.taxable_year, queued.quarter)
            .expect("the 2551Q lookup should succeed")
            .expect("the 2551Q snapshot should remain persisted")
    }

    /// One queue tick, then wait for the spawned 2551Q task to finish: it
    /// holds its job key until it returns.
    async fn tick_2551q(
        db: &Arc<Mutex<Database>>,
        transport: &Arc<RecordingSubmissionTransport>,
        queued: &Form2551QDraft,
    ) {
        run_queue_tick(db.clone(), transport).await;
        let key = format!(
            "form:2551Q:{}:{}:{}",
            queued.tin, queued.taxable_year, queued.quarter
        );
        for _ in 0..500 {
            if !get_active_jobs()
                .lock()
                .expect("the active job set should not be poisoned")
                .contains(&key)
            {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("the 2551Q queue task did not finish");
    }

    fn make_2551q_retry_due(db: &Arc<Mutex<Database>>, queued: &Form2551QDraft) {
        let stored = stored_2551q(db, queued);
        let mut due = stored.clone();
        due.next_retry_at = Some("1999-01-01T00:00:00Z".to_string());
        // The queue's own time is the one that rules: make it due too.
        due.smart_queue.next_try_at = due.next_retry_at.clone();
        due.smart_queue.ended_at = None;
        due.smart_queue.wait_until = None;
        assert!(
            db.lock()
                .expect("the cron database should not be poisoned")
                .replace_unclaimed_queued_2551q_submission(
                    &due,
                    &stored.queued_submission_fingerprint,
                    &stored.next_retry_at,
                    stored.submission_attempts,
                )
                .expect("forcing the retry timestamp should CAS-succeed")
        );
    }

    #[tokio::test]
    async fn queued_2551q_not_sent_at_store_releases_the_claim_and_retries_after_backoff() {
        let (queued, db) = saved_queued_2551q("801");
        let failing = Arc::new(RecordingSubmissionTransport::new(
            TestTransportOutcome::NotSentAtStore,
        ));
        tick_2551q(&db, &failing, &queued).await;

        assert_eq!(failing.calls().len(), 1);
        let failed = stored_2551q(&db, &queued);
        assert_eq!(failed.status, FilingStatus::Queued);
        assert_eq!(
            failed.queued_submission_fingerprint,
            queued.queued_submission_fingerprint
        );
        assert_eq!(failed.submission_attempts, 1);
        assert!(failed.submission_claim_token.is_none());
        assert!(failed.submission_claimed_at.is_none());
        assert!(failed.submission_claim_lease_until.is_none());
        assert!(failed.submission_put_started_at.is_none());
        let error = failed.last_error.as_deref().unwrap_or_default();
        assert!(error.starts_with(NOT_SENT_PREFIX), "{error}");
        assert!(
            retry_is_in_the_future(failed.next_retry_at.as_deref()),
            "{:?}",
            failed.next_retry_at
        );
        assert_eq!(
            form_drafts_status_column(
                &db,
                "2551Q",
                &queued.tin,
                queued.taxable_year,
                queued.quarter
            ),
            "Queued"
        );

        // Still inside the backoff: no second PUT.
        let early = Arc::new(RecordingSubmissionTransport::new(
            TestTransportOutcome::Success,
        ));
        tick_2551q(&db, &early, &queued).await;
        assert!(early.calls().is_empty());

        make_2551q_retry_due(&db, &queued);
        let retry = Arc::new(RecordingSubmissionTransport::new(
            TestTransportOutcome::Success,
        ));
        tick_2551q(&db, &retry, &queued).await;
        let calls = retry.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].filename, queued.default_submission_filename());
        let submitted = stored_2551q(&db, &queued);
        assert_eq!(submitted.status, FilingStatus::Sent);
        assert!(submitted.submission_claim_token.is_none());
    }

    #[tokio::test]
    async fn queued_2551q_not_sent_at_store_can_be_canceled() {
        // Seeded as the app does: the Draft is saved and reopened before it
        // is queued, so the stored JSON carries the row id that
        // `get_2551q_draft` sets and the exact-JSON cancel compares.
        let db = test_db();
        let profile = db
            .lock()
            .expect("the cron database should not be poisoned")
            .save_profile(reviewed_profile_with_tin("802"))
            .expect("the reviewed profile should persist");
        let mut draft = Form2551QDraft::new_from_effective_profile(&profile, 2024, 1);
        draft.item_13_election = Item13Election::Graduated;
        db.lock()
            .expect("the cron database should not be poisoned")
            .save_2551q_draft(&draft)
            .expect("the 2551Q draft should persist");
        let mut queued = stored_2551q(&db, &draft);
        queued
            .transition_to_queued()
            .expect("the reviewed draft should queue");
        db.lock()
            .expect("the cron database should not be poisoned")
            .save_queued_2551q_draft_and_election(&queued)
            .expect("the reviewed 2551Q queue snapshot should persist");
        let failing = Arc::new(RecordingSubmissionTransport::new(
            TestTransportOutcome::NotSentAtStore,
        ));
        tick_2551q(&db, &failing, &queued).await;
        assert_eq!(failing.calls().len(), 1);

        let failed = stored_2551q(&db, &queued);
        let canceled = db
            .lock()
            .expect("the cron database should not be poisoned")
            .cancel_queued_2551q_submission(&failed)
            .expect("a released not-sent row should cancel");
        assert_eq!(canceled.status, FilingStatus::Draft);
        assert_eq!(stored_2551q(&db, &queued).status, FilingStatus::Draft);
    }

    #[tokio::test]
    async fn queued_2551q_stays_queued_and_authorized_after_any_number_of_failures() {
        let (queued, db) = saved_queued_2551q("803");
        for attempt in 1..=7 {
            let transport = Arc::new(RecordingSubmissionTransport::new(
                TestTransportOutcome::NotSentAtStore,
            ));
            tick_2551q(&db, &transport, &queued).await;
            assert_eq!(transport.calls().len(), 1, "attempt {attempt}");
            assert_eq!(stored_2551q(&db, &queued).submission_attempts, attempt);
            make_2551q_retry_due(&db, &queued);
        }

        let waiting = stored_2551q(&db, &queued);
        assert_eq!(waiting.status, FilingStatus::Queued);
        assert_eq!(
            form_drafts_status_column(
                &db,
                "2551Q",
                &queued.tin,
                queued.taxable_year,
                queued.quarter
            ),
            "Queued"
        );
        assert_eq!(
            waiting.queued_submission_fingerprint,
            queued.queued_submission_fingerprint
        );
        assert!(waiting.queue_authorization.is_some());
        assert!(waiting.submission_claim_token.is_none());
        assert!(
            waiting
                .last_error
                .as_deref()
                .is_some_and(|error| error.starts_with(NOT_SENT_PREFIX))
        );

        let after = Arc::new(RecordingSubmissionTransport::new(
            TestTransportOutcome::Success,
        ));
        tick_2551q(&db, &after, &queued).await;
        assert_eq!(after.calls().len(), 1);
        assert_eq!(stored_2551q(&db, &queued).status, FilingStatus::Sent);
    }

    #[tokio::test]
    async fn generic_worker_not_sent_at_store_releases_the_claim_and_backs_off() {
        let profile = test_profile();
        let db = test_db();
        let (queued, summary) = queued_test_form(&db);
        let transport = RecordingSubmissionTransport::new(TestTransportOutcome::NotSentAtStore);

        process_queued_queueable::<TestForm, _>(
            &summary,
            &profile,
            db.clone(),
            &transport,
            allow_test_form,
        )
        .await;

        assert_eq!(transport.calls().len(), 1);
        let failed = stored_test_form(&db, &queued);
        assert_eq!(failed.lifecycle.status, FilingStatus::Queued);
        assert_eq!(failed.lifecycle.submission_attempts, 1);
        assert!(failed.lifecycle.is_unclaimed());
        assert!(failed.lifecycle.submission_put_started_at.is_none());
        let error = failed
            .lifecycle
            .submission_error
            .as_deref()
            .unwrap_or_default();
        assert!(error.starts_with(NOT_SENT_PREFIX), "{error}");
        assert!(retry_is_in_the_future(
            failed.lifecycle.next_retry_at.as_deref()
        ));

        // Still inside the backoff: no second PUT.
        let early = RecordingSubmissionTransport::new(TestTransportOutcome::Success);
        process_queued_queueable::<TestForm, _>(
            &summary,
            &profile,
            db.clone(),
            &early,
            allow_test_form,
        )
        .await;
        assert!(early.calls().is_empty());
    }

    #[tokio::test]
    async fn queued_2551q_unknown_outcome_stays_claimed_and_is_not_retried() {
        let (queued, db) = saved_queued_2551q("804");
        let failing = Arc::new(RecordingSubmissionTransport::new(
            TestTransportOutcome::UnknownIo,
        ));
        tick_2551q(&db, &failing, &queued).await;

        assert_eq!(failing.calls().len(), 1);
        let claimed = stored_2551q(&db, &queued);
        assert_eq!(claimed.status, FilingStatus::Queued);
        assert_eq!(claimed.submission_attempts, 0);
        assert!(claimed.submission_claim_token.is_some());
        assert!(claimed.submission_claimed_at.is_some());
        assert!(claimed.submission_put_started_at.is_some());
        assert!(
            claimed
                .last_error
                .as_deref()
                .is_some_and(|message| message.contains("outcome pending"))
        );
        assert_eq!(
            form_drafts_status_column(
                &db,
                "2551Q",
                &queued.tin,
                queued.taxable_year,
                queued.quarter
            ),
            "Queued"
        );

        // A later tick must not PUT again: the outcome is unknown.
        let retry = Arc::new(RecordingSubmissionTransport::new(
            TestTransportOutcome::Success,
        ));
        tick_2551q(&db, &retry, &queued).await;
        assert!(retry.calls().is_empty());
        assert_eq!(
            stored_2551q(&db, &queued).submission_claim_token,
            claimed.submission_claim_token
        );
    }

    /// The stored `(status, data_json)` of a row.
    fn raw_row(
        db: &Arc<Mutex<Database>>,
        form_code: &str,
        tin: &str,
        taxable_year: u16,
        slot: u8,
    ) -> (String, String) {
        db.lock()
            .expect("the cron database should not be poisoned")
            .conn
            .query_row(
                "SELECT status, data_json FROM form_drafts
                 WHERE tin = ?1 AND form_code = ?2 AND taxable_year = ?3 AND quarter = ?4",
                rusqlite::params![tin, form_code, i64::from(taxable_year), i64::from(slot)],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .expect("the form_drafts row should exist")
    }

    fn claim_stored_1601c(db: &Arc<Mutex<Database>>, queued: &Form1601CDraft) -> String {
        let stored = stored_1601c(db, queued);
        let claim = db
            .lock()
            .expect("the cron database should not be poisoned")
            .claim_queued_1601c_submission(
                &stored.tin,
                stored.taxable_year,
                stored.month,
                &stored.queued_submission_fingerprint,
                &stored.next_retry_at,
                stored.submission_attempts,
            )
            .expect("claim");
        let crate::db::Claim1601CSubmissionResult::Claimed { token, .. } = claim else {
            panic!("the 1601C should claim");
        };
        token
    }

    #[tokio::test]
    async fn a_stale_1601c_worker_cannot_fail_a_newer_claim() {
        let (_profile, queued, db, _summary) = saved_queued_1601c();
        let stale = claim_stored_1601c(&db, &queued);
        {
            let db_guard = db.lock().expect("the cron database should not be poisoned");
            assert!(matches!(
                db_guard
                    .release_abandoned_claimed_1601c_submission(
                        &queued.tin,
                        queued.taxable_year,
                        queued.month,
                        crate::db::ABANDONED_CLAIM_RELEASE_REASON,
                    )
                    .expect("release"),
                crate::db::AbandonedClaimRelease::Released { .. }
            ));
        }
        let mut requeued = stored_1601c(&db, &queued);
        requeued
            .transition_to_queued()
            .expect("the released 1601C should queue again");
        db.lock()
            .expect("the cron database should not be poisoned")
            .save_queued_1601c_draft(&requeued)
            .expect("requeue");
        let newer = claim_stored_1601c(&db, &queued);
        assert_ne!(stale, newer);
        let before = raw_row(&db, "1601C", &queued.tin, queued.taxable_year, queued.month);

        let failed = db
            .lock()
            .expect("the cron database should not be poisoned")
            .fail_claimed_1601c_not_sent(
                &queued.tin,
                queued.taxable_year,
                queued.month,
                &stale,
                format!("{NOT_SENT_PREFIX}stale worker"),
                &smart_queue::default_retry(0, Utc::now()),
            )
            .expect("the stale call should run");
        assert!(!failed);
        assert_eq!(
            raw_row(&db, "1601C", &queued.tin, queued.taxable_year, queued.month),
            before
        );
        assert_eq!(
            stored_1601c(&db, &queued).submission_claim_token.as_deref(),
            Some(newer.as_str())
        );
    }

    fn claim_stored_2551q(db: &Arc<Mutex<Database>>, queued: &Form2551QDraft) -> String {
        let stored = stored_2551q(db, queued);
        let claim = db
            .lock()
            .expect("the cron database should not be poisoned")
            .claim_queued_2551q_submission(
                &stored.tin,
                stored.taxable_year,
                stored.quarter,
                &stored.queued_submission_fingerprint,
                &stored.next_retry_at,
                stored.submission_attempts,
            )
            .expect("claim");
        let crate::db::Claim2551QSubmissionResult::Claimed { token, .. } = claim else {
            panic!("the 2551Q should claim");
        };
        token
    }

    #[tokio::test]
    async fn a_stale_2551q_worker_cannot_fail_a_newer_claim() {
        let (queued, db) = saved_queued_2551q("805");
        let stale = claim_stored_2551q(&db, &queued);
        {
            let db_guard = db.lock().expect("the cron database should not be poisoned");
            assert!(matches!(
                db_guard
                    .release_abandoned_claimed_2551q_submission(
                        &queued.tin,
                        queued.taxable_year,
                        queued.quarter,
                        crate::db::ABANDONED_CLAIM_RELEASE_REASON,
                    )
                    .expect("release"),
                crate::db::AbandonedClaimRelease::Released { .. }
            ));
        }
        let mut requeued = stored_2551q(&db, &queued);
        requeued
            .transition_to_queued()
            .expect("the released 2551Q should queue again");
        db.lock()
            .expect("the cron database should not be poisoned")
            .save_queued_2551q_draft_and_election(&requeued)
            .expect("requeue");
        let newer = claim_stored_2551q(&db, &queued);
        assert_ne!(stale, newer);
        let before = raw_row(
            &db,
            "2551Q",
            &queued.tin,
            queued.taxable_year,
            queued.quarter,
        );

        let failed = db
            .lock()
            .expect("the cron database should not be poisoned")
            .fail_claimed_2551q_not_sent(
                &queued.tin,
                queued.taxable_year,
                queued.quarter,
                &stale,
                format!("{NOT_SENT_PREFIX}stale worker"),
                &smart_queue::default_retry(0, Utc::now()),
            )
            .expect("the stale call should run");
        assert!(!failed);
        assert_eq!(
            raw_row(
                &db,
                "2551Q",
                &queued.tin,
                queued.taxable_year,
                queued.quarter
            ),
            before
        );
        assert_eq!(
            stored_2551q(&db, &queued).submission_claim_token.as_deref(),
            Some(newer.as_str())
        );
    }

    // ----- A TIN refusal goes back to Draft by claim token -----

    const REFUSED_UNVERIFIED: &str =
        "Not sent (nothing reached BIR): This TIN is not verified yet. (tin_unverified)";

    #[tokio::test]
    async fn a_stale_1601c_worker_cannot_refuse_a_newer_claim() {
        let (_profile, queued, db, _summary) = saved_queued_1601c();
        let stale = claim_stored_1601c(&db, &queued);
        {
            let db_guard = db.lock().expect("the cron database should not be poisoned");
            assert!(matches!(
                db_guard
                    .release_abandoned_claimed_1601c_submission(
                        &queued.tin,
                        queued.taxable_year,
                        queued.month,
                        crate::db::ABANDONED_CLAIM_RELEASE_REASON,
                    )
                    .expect("release"),
                crate::db::AbandonedClaimRelease::Released { .. }
            ));
        }
        let mut requeued = stored_1601c(&db, &queued);
        requeued
            .transition_to_queued()
            .expect("the released 1601C should queue again");
        db.lock()
            .expect("the cron database should not be poisoned")
            .save_queued_1601c_draft(&requeued)
            .expect("requeue");
        let newer = claim_stored_1601c(&db, &queued);
        let before = raw_row(&db, "1601C", &queued.tin, queued.taxable_year, queued.month);
        let refuse = |token: &str| {
            db.lock()
                .expect("the cron database should not be poisoned")
                .refuse_claimed_1601c_tin(
                    &queued.tin,
                    queued.taxable_year,
                    queued.month,
                    token,
                    REFUSED_UNVERIFIED.to_string(),
                )
                .expect("the refusal should run")
        };

        assert!(!refuse(&stale));
        assert_eq!(
            raw_row(&db, "1601C", &queued.tin, queued.taxable_year, queued.month),
            before
        );

        assert!(refuse(&newer));
        let refused = stored_1601c(&db, &queued);
        assert_eq!(refused.status, FilingStatus::Draft);
        assert_eq!(
            refused.submission_error.as_deref(),
            Some(REFUSED_UNVERIFIED)
        );
        assert!(refused.submission_claim_token.is_none());
        assert_eq!(
            form_drafts_status_column(&db, "1601C", &queued.tin, queued.taxable_year, queued.month),
            "Draft"
        );
    }

    #[tokio::test]
    async fn a_stale_2551q_worker_cannot_refuse_a_newer_claim() {
        let (queued, db) = saved_queued_2551q("806");
        let stale = claim_stored_2551q(&db, &queued);
        {
            let db_guard = db.lock().expect("the cron database should not be poisoned");
            assert!(matches!(
                db_guard
                    .release_abandoned_claimed_2551q_submission(
                        &queued.tin,
                        queued.taxable_year,
                        queued.quarter,
                        crate::db::ABANDONED_CLAIM_RELEASE_REASON,
                    )
                    .expect("release"),
                crate::db::AbandonedClaimRelease::Released { .. }
            ));
        }
        let mut requeued = stored_2551q(&db, &queued);
        requeued
            .transition_to_queued()
            .expect("the released 2551Q should queue again");
        db.lock()
            .expect("the cron database should not be poisoned")
            .save_queued_2551q_draft_and_election(&requeued)
            .expect("requeue");
        let newer = claim_stored_2551q(&db, &queued);
        let (tin, year, quarter) = (&queued.tin, queued.taxable_year, queued.quarter);
        let before = raw_row(&db, "2551Q", tin, year, quarter);
        let refuse = |token: &str| {
            db.lock()
                .expect("the cron database should not be poisoned")
                .refuse_claimed_2551q_tin(tin, year, quarter, token, REFUSED_UNVERIFIED.to_string())
                .expect("the refusal should run")
        };

        assert!(!refuse(&stale));
        assert_eq!(raw_row(&db, "2551Q", tin, year, quarter), before);

        assert!(refuse(&newer));
        let refused = stored_2551q(&db, &queued);
        assert_eq!(refused.status, FilingStatus::Draft);
        assert_eq!(refused.last_error.as_deref(), Some(REFUSED_UNVERIFIED));
        assert!(refused.submission_claim_token.is_none());
        assert_eq!(
            form_drafts_status_column(&db, "2551Q", tin, year, quarter),
            "Draft"
        );
    }

    fn claim_stored_test_form(db: &Arc<Mutex<Database>>, form: &TestForm) -> String {
        let stored = stored_test_form(db, form);
        let lifecycle = stored.lifecycle();
        let claim = db
            .lock()
            .unwrap()
            .claim_queued_queueable::<TestForm>(
                &stored.tin,
                stored.taxable_year,
                i64::from(stored.month),
                &lifecycle.queued_submission_fingerprint,
                &lifecycle.next_retry_at,
                lifecycle.submission_attempts,
            )
            .expect("claim");
        let crate::db::ClaimQueueableResult::Claimed { token, .. } = claim else {
            panic!("the test form should claim");
        };
        token
    }

    #[tokio::test]
    async fn a_stale_generic_worker_cannot_refuse_a_newer_claim() {
        let db = test_db();
        let (queued, _summary) = queued_test_form(&db);
        let (tin, year, period) = (&queued.tin, queued.taxable_year, i64::from(queued.month));
        let stale = claim_stored_test_form(&db, &queued);
        assert!(matches!(
            db.lock()
                .unwrap()
                .release_abandoned_claimed_queueable::<TestForm>(
                    tin,
                    year,
                    period,
                    crate::db::ABANDONED_CLAIM_RELEASE_REASON,
                )
                .expect("release"),
            crate::db::AbandonedClaimRelease::Released { .. }
        ));
        let mut requeued = stored_test_form(&db, &queued);
        requeued
            .queue(crate::filing_queue::QueueAuthSource::Gui)
            .expect("the released test form should queue again");
        db.lock()
            .unwrap()
            .save_queued_queueable(&requeued)
            .expect("requeue");
        let newer = claim_stored_test_form(&db, &queued);
        let before = raw_row(&db, TestForm::CODE, tin, year, queued.month);
        let refuse = |token: &str| {
            db.lock()
                .unwrap()
                .refuse_claimed_queueable_tin::<TestForm>(
                    tin,
                    year,
                    period,
                    token,
                    REFUSED_UNVERIFIED.to_string(),
                )
                .expect("the refusal should run")
        };

        assert!(!refuse(&stale));
        assert_eq!(
            raw_row(&db, TestForm::CODE, tin, year, queued.month),
            before
        );

        assert!(refuse(&newer));
        let refused = stored_test_form(&db, &queued);
        assert_eq!(refused.lifecycle.status, FilingStatus::Draft);
        assert_eq!(
            refused.lifecycle.submission_error.as_deref(),
            Some(REFUSED_UNVERIFIED)
        );
        assert!(refused.lifecycle.is_unclaimed());
        assert_eq!(
            form_drafts_status_column(&db, TestForm::CODE, tin, year, queued.month),
            "Draft"
        );
    }

    // ----- The worker sends a TIN refusal back to Draft at once -----

    const TIN_CODES: [&str; 6] = [
        "tin_not_active",
        "tin_unverified",
        "tin_pending",
        "tin_needs_fix",
        "tin_paused",
        "account_banned",
    ];

    /// The server's refusal for `code`, as the transport hands it over.
    fn refused_with(code: &str) -> &'static str {
        Box::leak(format!("Buwiz would not send this for its TIN ({code})").into_boxed_str())
    }

    fn refusal_alerts(db: &Arc<Mutex<Database>>, form_code: &str) -> usize {
        db.lock()
            .unwrap()
            .list_active_alerts(None)
            .unwrap()
            .iter()
            .filter(|alert| {
                alert
                    .kind
                    .starts_with(&format!("form_refused:{form_code}:"))
            })
            .count()
    }

    #[tokio::test]
    async fn process_queued_1601c_tin_refusal_returns_to_draft_at_once() {
        for code in TIN_CODES {
            let (profile, queued, db, summary) = saved_queued_1601c();
            let transport = RecordingSubmissionTransport::new(TestTransportOutcome::NotSentWith(
                refused_with(code),
            ));
            process_queued_1601c_with_transport(&summary, &profile, db.clone(), &transport).await;

            assert_eq!(transport.calls().len(), 1, "{code}");
            let refused = stored_1601c(&db, &queued);
            assert_eq!(refused.status, FilingStatus::Draft, "{code}");
            assert_eq!(
                form_drafts_status_column(
                    &db,
                    "1601C",
                    &queued.tin,
                    queued.taxable_year,
                    queued.month
                ),
                "Draft"
            );
            assert_eq!(refused.submission_attempts, 1);
            let error = refused.submission_error.as_deref().unwrap_or_default();
            assert!(error.starts_with(NOT_SENT_PREFIX), "{error}");
            assert!(error.ends_with(&format!("({code})")), "{error}");
            assert!(refused.submission_claim_token.is_none());
            assert!(refused.submission_claimed_at.is_none());
            assert!(refused.submission_claim_lease_until.is_none());
            assert!(refused.submission_put_started_at.is_none());
            assert!(refused.queued_submission_fingerprint.is_none());
            assert!(refused.queue_authorization.is_none());
            assert!(refused.submitted_payload.is_none());
            assert!(refused.next_retry_at.is_none());
            assert_eq!(refusal_alerts(&db, "1601C"), 1, "{code}");

            // A Draft is never sent by itself.
            let after = RecordingSubmissionTransport::new(TestTransportOutcome::Success);
            process_queued_1601c_with_transport(&summary, &profile, db.clone(), &after).await;
            assert!(after.calls().is_empty(), "{code}");

            // Queueing it again clears the reason.
            let mut requeued = stored_1601c(&db, &queued);
            requeued
                .transition_to_queued()
                .expect("the refused 1601C should queue again");
            db.lock()
                .unwrap()
                .save_queued_1601c_draft(&requeued)
                .expect("requeue");
            let requeued = stored_1601c(&db, &queued);
            assert_eq!(requeued.status, FilingStatus::Queued);
            assert_eq!(requeued.submission_attempts, 0);
            assert!(requeued.submission_error.is_none());
        }
    }

    #[tokio::test]
    async fn queued_2551q_tin_refusal_returns_to_draft_at_once() {
        for (code, segment3) in TIN_CODES
            .into_iter()
            .zip(["811", "812", "813", "814", "815", "816"])
        {
            let (queued, db) = saved_queued_2551q(segment3);
            let transport = Arc::new(RecordingSubmissionTransport::new(
                TestTransportOutcome::NotSentWith(refused_with(code)),
            ));
            tick_2551q(&db, &transport, &queued).await;

            assert_eq!(transport.calls().len(), 1, "{code}");
            let refused = stored_2551q(&db, &queued);
            assert_eq!(refused.status, FilingStatus::Draft, "{code}");
            assert_eq!(
                form_drafts_status_column(
                    &db,
                    "2551Q",
                    &queued.tin,
                    queued.taxable_year,
                    queued.quarter
                ),
                "Draft"
            );
            assert_eq!(refused.submission_attempts, 1);
            let error = refused.last_error.as_deref().unwrap_or_default();
            assert!(error.starts_with(NOT_SENT_PREFIX), "{error}");
            assert!(error.ends_with(&format!("({code})")), "{error}");
            assert!(refused.submission_claim_token.is_none());
            assert!(refused.submission_claimed_at.is_none());
            assert!(refused.submission_put_started_at.is_none());
            assert!(refused.queued_submission_fingerprint.is_none());
            assert!(refused.queue_authorization.is_none());
            assert!(refused.submitted_payload.is_none());
            assert!(refused.next_retry_at.is_none());
            assert_eq!(refusal_alerts(&db, "2551Q"), 1, "{code}");

            let after = Arc::new(RecordingSubmissionTransport::new(
                TestTransportOutcome::Success,
            ));
            tick_2551q(&db, &after, &queued).await;
            assert!(after.calls().is_empty(), "{code}");
        }
    }

    #[tokio::test]
    async fn generic_worker_tin_refusal_returns_to_draft_at_once() {
        for code in TIN_CODES {
            let profile = test_profile();
            let db = test_db();
            let (queued, summary) = queued_test_form(&db);
            let transport = RecordingSubmissionTransport::new(TestTransportOutcome::NotSentWith(
                refused_with(code),
            ));
            process_queued_queueable::<TestForm, _>(
                &summary,
                &profile,
                db.clone(),
                &transport,
                allow_test_form,
            )
            .await;

            assert_eq!(transport.calls().len(), 1, "{code}");
            let refused = stored_test_form(&db, &queued);
            let lifecycle = &refused.lifecycle;
            assert_eq!(lifecycle.status, FilingStatus::Draft, "{code}");
            assert_eq!(
                form_drafts_status_column(
                    &db,
                    TestForm::CODE,
                    &queued.tin,
                    queued.taxable_year,
                    queued.month
                ),
                "Draft"
            );
            assert_eq!(lifecycle.submission_attempts, 1);
            let error = lifecycle.submission_error.as_deref().unwrap_or_default();
            assert!(error.starts_with(NOT_SENT_PREFIX), "{error}");
            assert!(error.ends_with(&format!("({code})")), "{error}");
            assert!(lifecycle.is_unclaimed());
            assert!(lifecycle.submission_put_started_at.is_none());
            assert!(lifecycle.queued_submission_fingerprint.is_none());
            assert!(lifecycle.queue_authorization.is_none());
            assert!(lifecycle.submitted_payload.is_none());
            assert!(lifecycle.next_retry_at.is_none());
            assert_eq!(refusal_alerts(&db, TestForm::CODE), 1, "{code}");

            let after = RecordingSubmissionTransport::new(TestTransportOutcome::Success);
            process_queued_queueable::<TestForm, _>(
                &summary,
                &profile,
                db.clone(),
                &after,
                allow_test_form,
            )
            .await;
            assert!(after.calls().is_empty(), "{code}");
        }
    }

    fn refusal_alert(db: &Arc<Mutex<Database>>, form_code: &str) -> crate::db::AppAlert {
        let mut alerts: Vec<_> = db
            .lock()
            .unwrap()
            .list_active_alerts(None)
            .unwrap()
            .into_iter()
            .filter(|alert| {
                alert
                    .kind
                    .starts_with(&format!("form_refused:{form_code}:"))
            })
            .collect();
        assert_eq!(alerts.len(), 1);
        alerts.remove(0)
    }

    /// The alert's words are the queue's own: never the server's message,
    /// and for a banned account they name the account, not the TIN.
    #[tokio::test]
    async fn the_refusal_alert_names_the_account_for_a_ban_and_never_the_servers_message() {
        let check = |alert: crate::db::AppAlert, code: &str| {
            let shown = format!("{}\n{}", alert.title, alert.detail);
            assert!(!shown.contains("SERVER SENTENCE"), "{shown}");
            assert!(!shown.contains("999-888"), "{shown}");
            if code == "account_banned" {
                assert!(!shown.contains("for this TIN"), "{shown}");
                assert!(shown.contains("this Buwiz account is banned"), "{shown}");
            } else {
                assert!(shown.contains("for this TIN"), "{shown}");
            }
        };
        for code in TIN_CODES {
            let reason: &'static str = Box::leak(
                format!("SERVER SENTENCE about 000-999-888-000 ({code})").into_boxed_str(),
            );
            let (profile, _queued, db, summary) = saved_queued_1601c();
            let transport =
                RecordingSubmissionTransport::new(TestTransportOutcome::NotSentWith(reason));
            process_queued_1601c_with_transport(&summary, &profile, db.clone(), &transport).await;
            check(refusal_alert(&db, "1601C"), code);

            let profile = test_profile();
            let db = test_db();
            let (_queued, summary) = queued_test_form(&db);
            let transport =
                RecordingSubmissionTransport::new(TestTransportOutcome::SessionFailsWith(reason));
            process_queued_queueable::<TestForm, _>(
                &summary,
                &profile,
                db.clone(),
                &transport,
                allow_test_form,
            )
            .await;
            check(refusal_alert(&db, TestForm::CODE), code);
        }
        let reason = "SERVER SENTENCE about 000-999-888-000 (account_banned)";
        let (queued, db) = saved_queued_2551q("823");
        let transport = Arc::new(RecordingSubmissionTransport::new(
            TestTransportOutcome::NotSentWith(reason),
        ));
        tick_2551q(&db, &transport, &queued).await;
        check(refusal_alert(&db, "2551Q"), "account_banned");
    }

    /// A refusal whose server message is empty reaches the worker as
    /// `" (code)"`, and a relay `failed` error can be just `"(code)"`: each
    /// goes back to Draft at once, the same as a refusal with words.
    #[tokio::test]
    async fn a_refusal_with_no_message_returns_to_draft_at_once() {
        for (n, reason) in [" (tin_paused)", "(tin_paused)"].into_iter().enumerate() {
            let (profile, queued, db, summary) = saved_queued_1601c();
            let transport =
                RecordingSubmissionTransport::new(TestTransportOutcome::NotSentWith(reason));
            process_queued_1601c_with_transport(&summary, &profile, db.clone(), &transport).await;
            assert_eq!(transport.calls().len(), 1, "{reason:?}");
            let refused = stored_1601c(&db, &queued);
            assert_eq!(refused.status, FilingStatus::Draft, "{reason:?}");
            assert_eq!(refused.submission_attempts, 1);
            assert!(refused.next_retry_at.is_none());
            assert_eq!(refusal_alerts(&db, "1601C"), 1, "{reason:?}");

            let (queued, db) = saved_queued_2551q(["820", "821"][n]);
            let transport = Arc::new(RecordingSubmissionTransport::new(
                TestTransportOutcome::NotSentWith(reason),
            ));
            tick_2551q(&db, &transport, &queued).await;
            assert_eq!(transport.calls().len(), 1, "{reason:?}");
            let refused = stored_2551q(&db, &queued);
            assert_eq!(refused.status, FilingStatus::Draft, "{reason:?}");
            assert_eq!(refused.submission_attempts, 1);
            assert!(refused.next_retry_at.is_none());
            assert_eq!(refusal_alerts(&db, "2551Q"), 1, "{reason:?}");

            let profile = test_profile();
            let db = test_db();
            let (queued, summary) = queued_test_form(&db);
            let transport =
                RecordingSubmissionTransport::new(TestTransportOutcome::NotSentWith(reason));
            process_queued_queueable::<TestForm, _>(
                &summary,
                &profile,
                db.clone(),
                &transport,
                allow_test_form,
            )
            .await;
            assert_eq!(transport.calls().len(), 1, "{reason:?}");
            let refused = stored_test_form(&db, &queued);
            assert_eq!(refused.lifecycle.status, FilingStatus::Draft, "{reason:?}");
            assert_eq!(refused.lifecycle.submission_attempts, 1);
            assert!(refused.lifecycle.next_retry_at.is_none());
            assert_eq!(refusal_alerts(&db, TestForm::CODE), 1, "{reason:?}");
        }
    }

    /// A server that put pages of free text in a refusal fills neither the
    /// row nor the log: the stored reason is cut, and still ends with its
    /// code.
    #[tokio::test]
    async fn a_long_refusal_is_stored_cut_to_its_cap_with_its_code() {
        let long: &'static str = Box::leak(
            format!("{} (tin_needs_fix)", "Sent back for a fix: x".repeat(400)).into_boxed_str(),
        );
        let check = |stored: Option<&str>| {
            let stored = stored.unwrap_or_default();
            assert!(
                stored.chars().count() <= REFUSAL_REASON_CAP + " (account_banned)".len(),
                "{} characters",
                stored.chars().count()
            );
            assert!(stored.ends_with("\u{2026} (tin_needs_fix)"), "{stored}");
            assert_eq!(TinRefusal::from_reason(stored), Some(TinRefusal::NeedsFix));
        };

        let (profile, queued, db, summary) = saved_queued_1601c();
        let transport = RecordingSubmissionTransport::new(TestTransportOutcome::NotSentWith(long));
        process_queued_1601c_with_transport(&summary, &profile, db.clone(), &transport).await;
        let refused = stored_1601c(&db, &queued);
        assert_eq!(refused.status, FilingStatus::Draft);
        check(refused.submission_error.as_deref());

        let (queued, db) = saved_queued_2551q("822");
        let transport = Arc::new(RecordingSubmissionTransport::new(
            TestTransportOutcome::NotSentWith(long),
        ));
        tick_2551q(&db, &transport, &queued).await;
        let refused = stored_2551q(&db, &queued);
        assert_eq!(refused.status, FilingStatus::Draft);
        check(refused.last_error.as_deref());

        let profile = test_profile();
        let db = test_db();
        let (queued, summary) = queued_test_form(&db);
        let transport = RecordingSubmissionTransport::new(TestTransportOutcome::NotSentWith(long));
        process_queued_queueable::<TestForm, _>(
            &summary,
            &profile,
            db.clone(),
            &transport,
            allow_test_form,
        )
        .await;
        let refused = stored_test_form(&db, &queued);
        assert_eq!(refused.lifecycle.status, FilingStatus::Draft);
        check(refused.lifecycle.submission_error.as_deref());

        // At session open too.
        let (profile, queued, db, summary) = saved_queued_1601c();
        let transport =
            RecordingSubmissionTransport::new(TestTransportOutcome::SessionFailsWith(long));
        process_queued_1601c_with_transport(&summary, &profile, db.clone(), &transport).await;
        let refused = stored_1601c(&db, &queued);
        assert_eq!(refused.status, FilingStatus::Draft);
        check(refused.submission_error.as_deref());
    }

    #[tokio::test]
    async fn a_not_sent_that_is_not_a_tin_refusal_still_backs_off() {
        for reason in [
            "the server refused the relay (502 Bad Gateway) (relay_refused)",
            "this TIN is not active on your account (tin_not_active), said an old proxy",
            "the relay closed early",
        ] {
            let (profile, queued, db, summary) = saved_queued_1601c();
            let transport =
                RecordingSubmissionTransport::new(TestTransportOutcome::NotSentWith(reason));
            process_queued_1601c_with_transport(&summary, &profile, db.clone(), &transport).await;

            assert_eq!(transport.calls().len(), 1);
            let failed = stored_1601c(&db, &queued);
            assert_eq!(failed.status, FilingStatus::Queued, "{reason}");
            assert_eq!(failed.submission_attempts, 1);
            assert!(failed.submission_claim_token.is_none());
            assert!(retry_is_in_the_future(failed.next_retry_at.as_deref()));
            assert_eq!(refusal_alerts(&db, "1601C"), 0);
        }
    }

    #[tokio::test]
    async fn maybe_sent_with_a_tin_code_in_its_text_stays_claimed() {
        let reason = "the connection ended during the send (tin_not_active)";
        let (profile, queued, db, summary) = saved_queued_1601c();
        let transport =
            RecordingSubmissionTransport::new(TestTransportOutcome::MaybeSentWith(reason));
        process_queued_1601c_with_transport(&summary, &profile, db.clone(), &transport).await;
        assert_eq!(transport.calls().len(), 1);
        let claimed = stored_1601c(&db, &queued);
        assert_eq!(claimed.status, FilingStatus::Queued);
        assert_eq!(claimed.submission_attempts, 0);
        assert!(claimed.submission_claim_token.is_some());
        assert!(claimed.submission_put_started_at.is_some());
        let retry = RecordingSubmissionTransport::new(TestTransportOutcome::Success);
        process_queued_1601c_with_transport(&summary, &profile, db.clone(), &retry).await;
        assert!(retry.calls().is_empty());

        let (queued, db) = saved_queued_2551q("817");
        let transport = Arc::new(RecordingSubmissionTransport::new(
            TestTransportOutcome::MaybeSentWith(reason),
        ));
        tick_2551q(&db, &transport, &queued).await;
        assert_eq!(transport.calls().len(), 1);
        let claimed = stored_2551q(&db, &queued);
        assert_eq!(claimed.status, FilingStatus::Queued);
        assert!(claimed.submission_claim_token.is_some());
        assert!(claimed.submission_put_started_at.is_some());

        let profile = test_profile();
        let db = test_db();
        let (queued, summary) = queued_test_form(&db);
        let transport =
            RecordingSubmissionTransport::new(TestTransportOutcome::MaybeSentWith(reason));
        process_queued_queueable::<TestForm, _>(
            &summary,
            &profile,
            db.clone(),
            &transport,
            allow_test_form,
        )
        .await;
        let claimed = stored_test_form(&db, &queued);
        assert_eq!(claimed.lifecycle.status, FilingStatus::Queued);
        assert!(!claimed.lifecycle.is_unclaimed());
        assert!(claimed.lifecycle.submission_put_started_at.is_some());
    }

    #[tokio::test]
    async fn a_banned_account_at_session_open_returns_to_draft_without_a_put() {
        let reason = "This Buwiz account is banned: it can't send returns. (account_banned)";
        let (profile, queued, db, summary) = saved_queued_1601c();
        let transport =
            RecordingSubmissionTransport::new(TestTransportOutcome::SessionFailsWith(reason));
        process_queued_1601c_with_transport(&summary, &profile, db.clone(), &transport).await;
        assert!(transport.calls().is_empty());
        let refused = stored_1601c(&db, &queued);
        assert_eq!(refused.status, FilingStatus::Draft);
        assert_eq!(refused.submission_attempts, 1);
        let error = refused.submission_error.as_deref().unwrap_or_default();
        assert!(
            error.contains("no PUT attempted") && error.ends_with("(account_banned)"),
            "{error}"
        );
        assert_eq!(refusal_alerts(&db, "1601C"), 1);

        let (queued, db) = saved_queued_2551q("818");
        let transport = Arc::new(RecordingSubmissionTransport::new(
            TestTransportOutcome::SessionFailsWith(reason),
        ));
        tick_2551q(&db, &transport, &queued).await;
        assert!(transport.calls().is_empty());
        let refused = stored_2551q(&db, &queued);
        assert_eq!(refused.status, FilingStatus::Draft);
        assert_eq!(refused.submission_attempts, 1);
        assert!(
            refused
                .last_error
                .as_deref()
                .unwrap_or_default()
                .ends_with("(account_banned)")
        );

        let profile = test_profile();
        let db = test_db();
        let (queued, summary) = queued_test_form(&db);
        let transport =
            RecordingSubmissionTransport::new(TestTransportOutcome::SessionFailsWith(reason));
        process_queued_queueable::<TestForm, _>(
            &summary,
            &profile,
            db.clone(),
            &transport,
            allow_test_form,
        )
        .await;
        assert!(transport.calls().is_empty());
        let refused = stored_test_form(&db, &queued);
        assert_eq!(refused.lifecycle.status, FilingStatus::Draft);
        assert_eq!(refused.lifecycle.submission_attempts, 1);
    }

    /// Store `error` on the queued 1601C as an older build left it: a failed
    /// try still Queued, with its next retry far off.
    fn leave_1601c_failed(db: &Arc<Mutex<Database>>, queued: &Form1601CDraft, error: &str) {
        let stored = stored_1601c(db, queued);
        let mut failed = stored.clone();
        failed.submission_error = Some(error.to_string());
        failed.submission_attempts = 2;
        failed.next_retry_at = Some("2099-01-01T00:00:00Z".to_string());
        assert!(
            db.lock()
                .unwrap()
                .replace_unclaimed_queued_1601c_submission(
                    &failed,
                    &stored.queued_submission_fingerprint,
                    &stored.next_retry_at,
                    stored.submission_attempts,
                )
                .unwrap()
        );
    }

    const LEGACY_NOT_ACTIVE: &str =
        "Not sent (nothing reached BIR): this TIN is not active on your account (tin_not_active)";

    #[tokio::test]
    async fn an_already_queued_failed_row_with_a_legacy_tin_code_is_tried_now_and_goes_to_draft() {
        let (profile, queued, db, summary) = saved_queued_1601c();
        leave_1601c_failed(&db, &queued, LEGACY_NOT_ACTIVE);
        let transport = RecordingSubmissionTransport::new(TestTransportOutcome::NotSentWith(
            refused_with("tin_not_active"),
        ));
        process_queued_1601c_with_transport(&summary, &profile, db.clone(), &transport).await;
        assert_eq!(transport.calls().len(), 1);
        let refused = stored_1601c(&db, &queued);
        assert_eq!(refused.status, FilingStatus::Draft);
        assert_eq!(refused.submission_attempts, 3);

        // The twin: the TIN is active by now, so it sends.
        let (profile, queued, db, summary) = saved_queued_1601c();
        leave_1601c_failed(&db, &queued, LEGACY_NOT_ACTIVE);
        let transport = RecordingSubmissionTransport::new(TestTransportOutcome::Success);
        process_queued_1601c_with_transport(&summary, &profile, db.clone(), &transport).await;
        assert_eq!(transport.calls().len(), 1);
        assert_eq!(stored_1601c(&db, &queued).status, FilingStatus::Sent);

        // Any other failure keeps its backoff.
        let (profile, queued, db, summary) = saved_queued_1601c();
        leave_1601c_failed(
            &db,
            &queued,
            "Not sent (nothing reached BIR): the relay closed early",
        );
        let transport = RecordingSubmissionTransport::new(TestTransportOutcome::Success);
        process_queued_1601c_with_transport(&summary, &profile, db.clone(), &transport).await;
        assert!(transport.calls().is_empty());
        assert_eq!(stored_1601c(&db, &queued).status, FilingStatus::Queued);
    }

    #[tokio::test]
    async fn an_already_queued_2551q_or_generic_row_with_a_legacy_tin_code_is_tried_now() {
        let (queued, db) = saved_queued_2551q("819");
        let stored = stored_2551q(&db, &queued);
        let mut failed = stored.clone();
        failed.last_error = Some(LEGACY_NOT_ACTIVE.to_string());
        failed.submission_attempts = 2;
        failed.next_retry_at = Some("2099-01-01T00:00:00Z".to_string());
        assert!(
            db.lock()
                .unwrap()
                .replace_unclaimed_queued_2551q_submission(
                    &failed,
                    &stored.queued_submission_fingerprint,
                    &stored.next_retry_at,
                    stored.submission_attempts,
                )
                .unwrap()
        );
        let transport = Arc::new(RecordingSubmissionTransport::new(
            TestTransportOutcome::NotSentWith(refused_with("tin_pending")),
        ));
        tick_2551q(&db, &transport, &queued).await;
        assert_eq!(transport.calls().len(), 1);
        assert_eq!(stored_2551q(&db, &queued).status, FilingStatus::Draft);

        let profile = test_profile();
        let db = test_db();
        let (queued, summary) = queued_test_form(&db);
        let stored = stored_test_form(&db, &queued);
        let mut failed = stored.clone();
        failed.lifecycle.submission_error = Some(LEGACY_NOT_ACTIVE.to_string());
        failed.lifecycle.submission_attempts = 2;
        failed.lifecycle.next_retry_at = Some("2099-01-01T00:00:00Z".to_string());
        assert!(
            db.lock()
                .unwrap()
                .replace_unclaimed_queued_queueable(
                    &failed,
                    &stored.lifecycle.queued_submission_fingerprint,
                    &stored.lifecycle.next_retry_at,
                    stored.lifecycle.submission_attempts,
                )
                .unwrap()
        );
        let transport = RecordingSubmissionTransport::new(TestTransportOutcome::Success);
        process_queued_queueable::<TestForm, _>(
            &summary,
            &profile,
            db.clone(),
            &transport,
            allow_test_form,
        )
        .await;
        assert_eq!(transport.calls().len(), 1);
        assert_eq!(
            stored_test_form(&db, &queued).lifecycle.status,
            FilingStatus::Sent
        );
    }

    /// What `a_not_sent_that_is_not_a_tin_refusal_still_backs_off` shows for
    /// the 1601C, for the 2551Q and the generic worker: a code that is not a
    /// TIN refusal, or one mid-sentence, keeps the backoff, and the next tick
    /// sends nothing.
    #[tokio::test]
    async fn a_2551q_or_generic_not_sent_that_is_not_a_tin_refusal_still_backs_off() {
        for (reason, segment3) in [
            (
                "the server refused the relay (502 Bad Gateway) (relay_refused)",
                "824",
            ),
            (
                "this TIN is not active on your account (tin_not_active), said an old proxy",
                "825",
            ),
            ("the relay closed early", "826"),
        ] {
            let (queued, db) = saved_queued_2551q(segment3);
            let transport = Arc::new(RecordingSubmissionTransport::new(
                TestTransportOutcome::NotSentWith(reason),
            ));
            tick_2551q(&db, &transport, &queued).await;
            assert_eq!(transport.calls().len(), 1, "{reason}");
            let failed = stored_2551q(&db, &queued);
            assert_eq!(failed.status, FilingStatus::Queued, "{reason}");
            assert_eq!(failed.submission_attempts, 1);
            assert!(failed.submission_claim_token.is_none());
            assert!(retry_is_in_the_future(failed.next_retry_at.as_deref()));
            assert_eq!(refusal_alerts(&db, "2551Q"), 0);
            let early = Arc::new(RecordingSubmissionTransport::new(
                TestTransportOutcome::Success,
            ));
            tick_2551q(&db, &early, &queued).await;
            assert!(early.calls().is_empty(), "{reason}");

            let profile = test_profile();
            let db = test_db();
            let (queued, summary) = queued_test_form(&db);
            let transport =
                RecordingSubmissionTransport::new(TestTransportOutcome::NotSentWith(reason));
            process_queued_queueable::<TestForm, _>(
                &summary,
                &profile,
                db.clone(),
                &transport,
                allow_test_form,
            )
            .await;
            assert_eq!(transport.calls().len(), 1, "{reason}");
            let failed = stored_test_form(&db, &queued);
            assert_eq!(failed.lifecycle.status, FilingStatus::Queued, "{reason}");
            assert_eq!(failed.lifecycle.submission_attempts, 1);
            assert!(failed.lifecycle.is_unclaimed());
            assert!(retry_is_in_the_future(
                failed.lifecycle.next_retry_at.as_deref()
            ));
            assert_eq!(refusal_alerts(&db, TestForm::CODE), 0);
            let early = RecordingSubmissionTransport::new(TestTransportOutcome::Success);
            process_queued_queueable::<TestForm, _>(
                &summary,
                &profile,
                db.clone(),
                &early,
                allow_test_form,
            )
            .await;
            assert!(early.calls().is_empty(), "{reason}");
        }
    }

    /// The generic worker stores a maybe-sent reason on the claimed row,
    /// and that reason ends with the code here: the row is still not tried
    /// again on the next tick, whatever its text says.
    #[tokio::test]
    async fn a_generic_maybe_sent_ending_in_a_tin_code_is_not_retried_on_the_next_tick() {
        let reason = "the connection ended during the send (tin_not_active)";
        let profile = test_profile();
        let db = test_db();
        let (queued, summary) = queued_test_form(&db);
        let transport =
            RecordingSubmissionTransport::new(TestTransportOutcome::MaybeSentWith(reason));
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
        assert!(!claimed.lifecycle.is_unclaimed());
        assert_eq!(claimed.lifecycle.status, FilingStatus::Queued);

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
        let after = stored_test_form(&db, &queued);
        assert!(!after.lifecycle.is_unclaimed());
        assert_eq!(after.lifecycle.status, FilingStatus::Queued);
        assert_eq!(refusal_alerts(&db, TestForm::CODE), 0);
    }

    /// A legacy row whose reason ends with a TIN code is tried at once. When
    /// that try fails for another reason, the new reason has no code, so the
    /// row is back on its backoff: tried once, not on every tick.
    #[tokio::test]
    async fn a_legacy_coded_row_that_fails_otherwise_goes_back_on_its_backoff() {
        let (profile, queued, db, summary) = saved_queued_1601c();
        leave_1601c_failed(&db, &queued, LEGACY_NOT_ACTIVE);
        let transport = RecordingSubmissionTransport::new(TestTransportOutcome::NotSentAtStore);
        process_queued_1601c_with_transport(&summary, &profile, db.clone(), &transport).await;
        assert_eq!(transport.calls().len(), 1);
        let failed = stored_1601c(&db, &queued);
        assert_eq!(failed.status, FilingStatus::Queued);
        assert_eq!(failed.submission_attempts, 3);
        assert!(retry_is_in_the_future(failed.next_retry_at.as_deref()));
        let next = RecordingSubmissionTransport::new(TestTransportOutcome::Success);
        process_queued_1601c_with_transport(&summary, &profile, db.clone(), &next).await;
        assert!(next.calls().is_empty(), "tried once, not on every tick");

        // A session that fails before the PUT does the same.
        let (profile, queued, db, summary) = saved_queued_1601c();
        leave_1601c_failed(&db, &queued, LEGACY_NOT_ACTIVE);
        let transport = RecordingSubmissionTransport::new(TestTransportOutcome::PreStore);
        process_queued_1601c_with_transport(&summary, &profile, db.clone(), &transport).await;
        let failed = stored_1601c(&db, &queued);
        assert_eq!(failed.status, FilingStatus::Queued);
        assert_eq!(failed.submission_attempts, 3);
        assert!(retry_is_in_the_future(failed.next_retry_at.as_deref()));
        let next = RecordingSubmissionTransport::new(TestTransportOutcome::Success);
        process_queued_1601c_with_transport(&summary, &profile, db.clone(), &next).await;
        assert!(next.calls().is_empty(), "tried once, not on every tick");

        let (queued, db) = saved_queued_2551q("827");
        let stored = stored_2551q(&db, &queued);
        let mut legacy = stored.clone();
        legacy.last_error = Some(LEGACY_NOT_ACTIVE.to_string());
        legacy.submission_attempts = 2;
        legacy.next_retry_at = Some("2099-01-01T00:00:00Z".to_string());
        assert!(
            db.lock()
                .unwrap()
                .replace_unclaimed_queued_2551q_submission(
                    &legacy,
                    &stored.queued_submission_fingerprint,
                    &stored.next_retry_at,
                    stored.submission_attempts,
                )
                .unwrap()
        );
        let transport = Arc::new(RecordingSubmissionTransport::new(
            TestTransportOutcome::NotSentAtStore,
        ));
        tick_2551q(&db, &transport, &queued).await;
        assert_eq!(transport.calls().len(), 1);
        let failed = stored_2551q(&db, &queued);
        assert_eq!(failed.status, FilingStatus::Queued);
        assert!(retry_is_in_the_future(failed.next_retry_at.as_deref()));
        let next = Arc::new(RecordingSubmissionTransport::new(
            TestTransportOutcome::Success,
        ));
        tick_2551q(&db, &next, &queued).await;
        assert!(next.calls().is_empty(), "tried once, not on every tick");

        let profile = test_profile();
        let db = test_db();
        let (queued, summary) = queued_test_form(&db);
        let stored = stored_test_form(&db, &queued);
        let mut legacy = stored.clone();
        legacy.lifecycle.submission_error = Some(LEGACY_NOT_ACTIVE.to_string());
        legacy.lifecycle.submission_attempts = 2;
        legacy.lifecycle.next_retry_at = Some("2099-01-01T00:00:00Z".to_string());
        assert!(
            db.lock()
                .unwrap()
                .replace_unclaimed_queued_queueable(
                    &legacy,
                    &stored.lifecycle.queued_submission_fingerprint,
                    &stored.lifecycle.next_retry_at,
                    stored.lifecycle.submission_attempts,
                )
                .unwrap()
        );
        let transport = RecordingSubmissionTransport::new(TestTransportOutcome::NotSentAtStore);
        let run = |transport| {
            process_queued_queueable::<TestForm, _>(
                &summary,
                &profile,
                db.clone(),
                transport,
                allow_test_form,
            )
        };
        run(&transport).await;
        assert_eq!(transport.calls().len(), 1);
        let failed = stored_test_form(&db, &queued);
        assert_eq!(failed.lifecycle.status, FilingStatus::Queued);
        assert!(retry_is_in_the_future(
            failed.lifecycle.next_retry_at.as_deref()
        ));
        let next = RecordingSubmissionTransport::new(TestTransportOutcome::Success);
        run(&next).await;
        assert!(next.calls().is_empty(), "tried once, not on every tick");
    }

    // ===== The smart queue: one at a time, the owner's waits, stay queued =====

    use crate::smart_queue::SeededRng;
    use std::sync::atomic::AtomicUsize;

    /// How the simulated far side answers.
    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Far {
        /// The BIR is down: every try fails at the front desk, nothing
        /// reached it, no stream opened: the way in is down for everyone.
        Outage,
        /// An upload stream opened and the send broke after it: this
        /// return's own failure (the way in works).
        UploadFails,
        /// The login was refused after the stream opened: about this
        /// return alone.
        LoginRefused,
        /// The BIR has no upload service for this TIN.
        NoService,
        /// The relay said class `ours` and no stream opened: not a try.
        OursNothingOpened,
        /// Class `ours`, but a stream opened: a counted failure.
        OursAfterOpening,
        /// The send never answers (a hung relay).
        Hang,
        /// The BIR takes it.
        Up,
        /// Buwiz holds the ticket until this time: not a try.
        Hold(DateTime<Utc>),
        /// The outcome is unknown.
        Unknown,
        /// The session fails before any ticket (signed out).
        SignedOut,
        /// The server answers the session with a refusal (a revoked key).
        SessionRefused,
    }

    const SESSION_REFUSED: &str = "This device may not send returns. (device_not_allowed)";

    /// A transport that answers as `far` says, records each try's simulated
    /// time and file, and counts sends in flight.
    struct SimTransport {
        far: Mutex<Far>,
        clock: Arc<Mutex<DateTime<Utc>>>,
        tries: Mutex<Vec<(String, DateTime<Utc>)>>,
        in_flight: AtomicUsize,
        most_in_flight: AtomicUsize,
        sign: Mutex<Option<SeenSign>>,
        session_not_a_try: bool,
    }

    impl SimTransport {
        fn new(far: Far, clock: &Arc<Mutex<DateTime<Utc>>>) -> Arc<Self> {
            Arc::new(Self {
                far: Mutex::new(far),
                clock: clock.clone(),
                tries: Mutex::new(Vec::new()),
                in_flight: AtomicUsize::new(0),
                most_in_flight: AtomicUsize::new(0),
                sign: Mutex::new(None),
                session_not_a_try: true,
            })
        }

        fn set(&self, far: Far) {
            *self.far.lock().unwrap() = far;
        }

        fn tries(&self) -> Vec<(String, DateTime<Utc>)> {
            self.tries.lock().unwrap().clone()
        }

        fn tries_of(&self, filename: &str) -> usize {
            self.tries()
                .iter()
                .filter(|(file, _)| file == filename)
                .count()
        }

        fn most_in_flight(&self) -> usize {
            self.most_in_flight.load(Ordering::SeqCst)
        }
    }

    impl SubmissionTransport for SimTransport {
        type Session = ();

        fn open_session<'a>(
            &'a self,
            _form_type: &'a str,
            _tin: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<(), TransportError>> + Send + 'a>> {
            Box::pin(async move {
                if *self.far.lock().unwrap() == Far::SignedOut {
                    return Err(TransportError("Sign in to send returns.".into()));
                }
                if *self.far.lock().unwrap() == Far::SessionRefused {
                    return Err(TransportError(SESSION_REFUSED.into()));
                }
                let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                self.most_in_flight.fetch_max(now, Ordering::SeqCst);
                Ok(())
            })
        }

        fn store_session<'a>(
            &'a self,
            _session: (),
            _filename: &'a str,
            _payload: &'a [u8],
        ) -> Pin<Box<dyn Future<Output = Result<(), TransportError>> + Send + 'a>> {
            Box::pin(async { unreachable!("the queue sends through store_session_reported") })
        }

        fn store_session_reported<'a>(
            &'a self,
            _session: (),
            filename: &'a str,
            _payload: &'a [u8],
        ) -> Pin<Box<dyn Future<Output = Result<StoreReport, StoreFailure>> + Send + 'a>> {
            Box::pin(async move {
                let at = *self.clock.lock().unwrap();
                self.tries.lock().unwrap().push((filename.to_owned(), at));
                if *self.far.lock().unwrap() == Far::Hang {
                    std::future::pending::<()>().await;
                }
                // Let a second tick run if it could.
                tokio::time::sleep(std::time::Duration::from_millis(2)).await;
                self.in_flight.fetch_sub(1, Ordering::SeqCst);
                match *self.far.lock().unwrap() {
                    Far::Up => Ok(StoreReport::default()),
                    Far::Outage => Err(StoreFailure::Counted {
                        error: TransportError("the BIR's mailbox did not answer".into()),
                        blame: Blame::Path,
                    }),
                    Far::SignedOut | Far::UploadFails | Far::OursAfterOpening => {
                        Err(StoreFailure::NotSent(TransportError(
                            "the BIR's mailbox did not answer".into(),
                        )))
                    }
                    Far::LoginRefused => Err(StoreFailure::Counted {
                        error: TransportError("the login was refused".into()),
                        blame: Blame::Return,
                    }),
                    Far::NoService => Err(StoreFailure::Counted {
                        error: TransportError(
                            "dispatcher has no upload server for this TIN (mode 0)".into(),
                        ),
                        blame: Blame::NoService,
                    }),
                    Far::OursNothingOpened => Err(StoreFailure::NotATry {
                        error: TransportError("Buwiz had a problem on its side (internal)".into()),
                        wait_until: None,
                    }),
                    Far::Hang | Far::SessionRefused => unreachable!("no send follows"),
                    Far::Hold(until) => Err(StoreFailure::NotATry {
                        error: TransportError(
                            "The BIR is offline, so Buwiz held this return. (bir_offline)".into(),
                        ),
                        wait_until: Some(until),
                    }),
                    Far::Unknown => Err(StoreFailure::MaybeSent(TransportError(
                        "the connection ended during the send".into(),
                    ))),
                }
            })
        }

        fn session_failure_class(&self, error: &TransportError) -> TryClass {
            if error.0 == SESSION_REFUSED {
                TryClass::Answered
            } else if self.session_not_a_try {
                TryClass::NotATry
            } else {
                TryClass::Counted
            }
        }

        fn sign(&self) -> Option<SeenSign> {
            self.sign.lock().unwrap().clone()
        }
    }

    /// A database holding one saved profile (TIN segment `segment3`) and a
    /// queued 1601C for each of `months` of 2024; the drafts, in order.
    fn queued_1601cs(segment3: &str, months: &[u8]) -> (Arc<Mutex<Database>>, Vec<Form1601CDraft>) {
        let db = test_db();
        let profile = db
            .lock()
            .unwrap()
            .save_profile(reviewed_profile_with_tin(segment3))
            .expect("the profile should persist");
        let drafts = months
            .iter()
            .map(|&month| {
                let mut draft = Form1601CDraft::new_from_profile(&profile, 2024, month);
                draft.any_taxes_withheld = false;
                draft
                    .transition_to_queued()
                    .expect("the 1601C should queue");
                db.lock()
                    .unwrap()
                    .save_queued_1601c_draft(&draft)
                    .expect("persist");
                draft
            })
            .collect();
        (db, drafts)
    }

    /// A clock the test moves, starting now, and an env on it.
    fn sim_clock(seed: u64) -> (Arc<Mutex<DateTime<Utc>>>, QueueEnv) {
        let clock = Arc::new(Mutex::new(Utc::now()));
        let read = clock.clone();
        (
            clock,
            QueueEnv::new(move || *read.lock().unwrap(), SeededRng::new(seed)),
        )
    }

    fn set_clock(clock: &Arc<Mutex<DateTime<Utc>>>, at: DateTime<Utc>) {
        *clock.lock().unwrap() = at;
    }

    fn stored_next(db: &Arc<Mutex<Database>>, draft: &Form1601CDraft) -> Option<DateTime<Utc>> {
        stored_1601c(db, draft)
            .next_retry_at
            .as_deref()
            .and_then(smart_queue::parse_time)
    }

    /// Ticks once a minute from `from` for `minutes`, as the worker does.
    async fn run_minutes(
        db: &Arc<Mutex<Database>>,
        transport: &Arc<SimTransport>,
        env: &QueueEnv,
        clock: &Arc<Mutex<DateTime<Utc>>>,
        from: DateTime<Utc>,
        minutes: i64,
    ) {
        for minute in 0..minutes {
            set_clock(clock, from + chrono::Duration::minutes(minute));
            run_queue_tick_with(db.clone(), transport, env).await;
        }
    }

    #[tokio::test]
    async fn one_tick_sends_only_the_head_due_soonest_and_stamps_the_rest() {
        let (db, drafts) = queued_1601cs("901", &[7, 5, 6]);
        let (clock, env) = sim_clock(1);
        let transport = SimTransport::new(Far::Up, &clock);
        let start = *clock.lock().unwrap();
        assert_eq!(
            run_queue_tick_with(db.clone(), &transport, &env).await,
            Some(SendOutcome::Sent)
        );
        let tries = transport.tries();
        assert_eq!(tries.len(), 1, "one return per tick");
        assert_eq!(
            tries[0].0,
            drafts[1].default_submission_filename(),
            "May is due first"
        );
        // The two behind it wait their turn a minute apart, never null.
        let june = stored_next(&db, &drafts[2]).expect("a real next try");
        let july = stored_next(&db, &drafts[0]).expect("a real next try");
        assert!(june >= start + chrono::Duration::seconds(60), "{june}");
        assert!(july >= june + chrono::Duration::seconds(60), "{july}");
        // After a landing the next goes at once (the wake), not a minute on.
        set_clock(&clock, start + chrono::Duration::seconds(1));
        run_queue_tick_with(db.clone(), &transport, &env).await;
        assert_eq!(
            transport.tries()[1].0,
            drafts[2].default_submission_filename()
        );
        run_queue_tick_with(db.clone(), &transport, &env).await;
        assert_eq!(
            transport.tries()[2].0,
            drafts[0].default_submission_filename()
        );
        assert!(
            drafts
                .iter()
                .all(|d| stored_1601c(&db, d).status == FilingStatus::Sent)
        );
        assert_eq!(
            run_queue_tick_with(db.clone(), &transport, &env).await,
            None,
            "nothing left"
        );
    }

    #[tokio::test]
    async fn the_head_is_due_soonest_across_every_tin() {
        let (db, drafts) = queued_1601cs("902", &[8]);
        let other = db
            .lock()
            .unwrap()
            .save_profile(reviewed_profile_with_tin("903"))
            .unwrap();
        let mut earlier = Form1601CDraft::new_from_profile(&other, 2024, 3);
        earlier.any_taxes_withheld = false;
        earlier.transition_to_queued().unwrap();
        db.lock()
            .unwrap()
            .save_queued_1601c_draft(&earlier)
            .unwrap();
        let (clock, env) = sim_clock(2);
        let transport = SimTransport::new(Far::Outage, &clock);
        run_queue_tick_with(db.clone(), &transport, &env).await;
        assert_eq!(transport.tries().len(), 1);
        assert_eq!(
            transport.tries()[0].0,
            earlier.default_submission_filename(),
            "another TIN's March first"
        );
        assert!(stored_next(&db, &drafts[0]).is_some());
    }

    #[tokio::test]
    async fn a_return_that_failed_steps_aside_for_the_next() {
        let (db, drafts) = queued_1601cs("904", &[5, 6]);
        let (clock, env) = sim_clock(3);
        let transport = SimTransport::new(Far::UploadFails, &clock);
        let start = *clock.lock().unwrap();
        run_queue_tick_with(db.clone(), &transport, &env).await;
        set_clock(&clock, start + chrono::Duration::minutes(1));
        assert_eq!(
            run_queue_tick_with(db.clone(), &transport, &env).await,
            Some(SendOutcome::Failed(TryClass::Counted))
        );
        let files: Vec<String> = transport
            .tries()
            .into_iter()
            .map(|(file, _)| file)
            .collect();
        assert_eq!(
            files,
            [
                drafts[0].default_submission_filename(),
                drafts[1].default_submission_filename()
            ]
        );
        // Both wait their own first wait now; nothing goes before it.
        set_clock(&clock, start + chrono::Duration::minutes(2));
        assert_eq!(
            run_queue_tick_with(db.clone(), &transport, &env).await,
            None
        );
        for draft in &drafts {
            let stored = stored_1601c(&db, draft);
            assert_eq!(stored.status, FilingStatus::Queued);
            assert_eq!(stored.smart_queue.failures, 1);
            let next = smart_queue::parse_time(stored.next_retry_at.as_deref().unwrap()).unwrap();
            assert!(
                next >= start + chrono::Duration::seconds(150)
                    && next <= start + chrono::Duration::seconds(510),
                "{next}"
            );
        }
    }

    #[tokio::test]
    async fn waking_from_sleep_or_a_clock_jump_sends_only_the_head() {
        let (db, drafts) = queued_1601cs("905", &[5, 6, 7]);
        let (clock, env) = sim_clock(4);
        let transport = SimTransport::new(Far::UploadFails, &clock);
        let start = *clock.lock().unwrap();
        run_minutes(&db, &transport, &env, &clock, start, 3).await;
        assert_eq!(transport.tries().len(), 3);
        // Ten hours later every return is long due: one goes.
        set_clock(&clock, start + chrono::Duration::hours(10));
        run_queue_tick_with(db.clone(), &transport, &env).await;
        assert_eq!(transport.tries().len(), 4, "at most the head goes on wake");
        // The clock goes back a day: nobody is stranded past the longest wait.
        set_clock(&clock, start - chrono::Duration::days(1));
        run_queue_tick_with(db.clone(), &transport, &env).await;
        assert_eq!(transport.tries().len(), 4);
        for draft in &drafts {
            assert!(stored_next(&db, draft).is_some());
        }
    }

    #[tokio::test]
    async fn two_ticks_at_once_never_send_two_returns() {
        let (db, _drafts) = queued_1601cs("906", &[5, 6, 7]);
        let (clock, env) = sim_clock(5);
        let transport = SimTransport::new(Far::Outage, &clock);
        let (a, b) = tokio::join!(
            run_queue_tick_with(db.clone(), &transport, &env),
            run_queue_tick_with(db.clone(), &transport, &env)
        );
        assert!(a.is_none() || b.is_none(), "{a:?} {b:?}");
        assert_eq!(transport.tries().len(), 1);
        assert_eq!(transport.most_in_flight(), 1);
    }

    #[tokio::test]
    async fn pause_stops_the_queue_and_auto_sends_again() {
        let (db, drafts) = queued_1601cs("907", &[5]);
        let (clock, env) = sim_clock(6);
        let transport = SimTransport::new(Far::Up, &clock);
        db.lock()
            .unwrap()
            .set_setting(smart_queue::QUEUE_MODE_SETTING, "paused")
            .unwrap();
        assert_eq!(
            run_queue_tick_with(db.clone(), &transport, &env).await,
            None
        );
        assert!(transport.tries().is_empty());
        assert_eq!(stored_1601c(&db, &drafts[0]).status, FilingStatus::Queued);
        db.lock()
            .unwrap()
            .set_setting(smart_queue::QUEUE_MODE_SETTING, "auto")
            .unwrap();
        assert_eq!(
            run_queue_tick_with(db.clone(), &transport, &env).await,
            Some(SendOutcome::Sent)
        );
    }

    #[tokio::test]
    async fn a_hold_is_not_a_try_and_nothing_goes_before_its_time() {
        let (db, drafts) = queued_1601cs("908", &[5]);
        let (clock, env) = sim_clock(7);
        let start = *clock.lock().unwrap();
        let until = start + chrono::Duration::hours(2);
        let transport = SimTransport::new(Far::Hold(until), &clock);
        assert_eq!(
            run_queue_tick_with(db.clone(), &transport, &env).await,
            Some(SendOutcome::Failed(TryClass::NotATry))
        );
        let held = stored_1601c(&db, &drafts[0]);
        assert_eq!(
            (
                held.status,
                held.submission_attempts,
                held.smart_queue.failures
            ),
            (FilingStatus::Queued, 0, 0)
        );
        assert!(
            held.submission_error
                .as_deref()
                .unwrap()
                .ends_with("(bir_offline)")
        );
        let next = stored_next(&db, &drafts[0]).unwrap();
        assert!(
            next >= until && next <= until + chrono::Duration::hours(1),
            "{next}"
        );
        run_minutes(
            &db,
            &transport,
            &env,
            &clock,
            start + chrono::Duration::minutes(1),
            119,
        )
        .await;
        assert_eq!(transport.tries().len(), 1, "nothing before wait_until");
    }

    #[tokio::test]
    async fn signed_out_is_not_a_try() {
        let (db, drafts) = queued_1601cs("909", &[5]);
        let (clock, env) = sim_clock(8);
        let transport = SimTransport::new(Far::SignedOut, &clock);
        assert_eq!(
            run_queue_tick_with(db.clone(), &transport, &env).await,
            Some(SendOutcome::Failed(TryClass::NotATry))
        );
        let waiting = stored_1601c(&db, &drafts[0]);
        assert_eq!(
            (waiting.submission_attempts, waiting.smart_queue.failures),
            (0, 0)
        );
        assert!(
            transport.tries().is_empty(),
            "no ticket, nothing toward the BIR"
        );
        assert_eq!(
            DeviceTries::from_setting(""),
            device_tries(&db),
            "not a try is not on the device's record"
        );
    }

    fn device_tries(db: &Arc<Mutex<Database>>) -> DeviceTries {
        db.lock()
            .unwrap()
            .get_setting(smart_queue::DEVICE_TRIES_SETTING)
            .unwrap()
            .map(|stored| DeviceTries::from_setting(&stored))
            .unwrap_or_default()
    }

    #[tokio::test]
    async fn the_device_guard_slows_down_a_device_that_cannot_get_in() {
        let (db, drafts) = queued_1601cs("910", &[5, 6, 7]);
        let (clock, env) = sim_clock(9);
        let transport = SimTransport::new(Far::UploadFails, &clock);
        let start = *clock.lock().unwrap();
        let online = crate::bir_sign::Sign::from_header("v=1, seq=5, state=online").unwrap();
        *transport.sign.lock().unwrap() = Some(SeenSign {
            sign: online,
            seen_at: start,
        });
        run_minutes(&db, &transport, &env, &clock, start, 3).await;
        assert_eq!(device_tries(&db).to_setting(), "xxx");
        let first = stored_1601c(&db, &drafts[0]);
        let third = stored_1601c(&db, &drafts[2]);
        assert!(!first.smart_queue.slowed);
        assert!(
            third.smart_queue.slowed,
            "the third failed try in a row while others get in"
        );
        let next = stored_next(&db, &drafts[2]).unwrap();
        assert!(next >= start + chrono::Duration::hours(1), "{next}");
        assert_eq!(third.status, FilingStatus::Queued);
        // Without the sign the same three failures are plain failures.
        let (db, drafts) = queued_1601cs("911", &[5, 6, 7]);
        let transport = SimTransport::new(Far::UploadFails, &clock);
        run_minutes(&db, &transport, &env, &clock, start, 3).await;
        assert!(!stored_1601c(&db, &drafts[2]).smart_queue.slowed);
    }

    #[tokio::test]
    async fn a_return_still_waiting_after_24_hours_is_noticed_once_a_day_and_stays_queued() {
        let (db, drafts) = queued_1601cs("912", &[5]);
        let (clock, env) = sim_clock(10);
        let transport = SimTransport::new(Far::Outage, &clock);
        let start = *clock.lock().unwrap();
        let notices = |db: &Arc<Mutex<Database>>| -> Vec<crate::db::AppAlert> {
            db.lock()
                .unwrap()
                .list_active_alerts(None)
                .unwrap()
                .into_iter()
                .filter(|alert| alert.kind.starts_with("queue_still_waiting:1601C:"))
                .collect()
        };
        for hour in [1, 23] {
            set_clock(&clock, start + chrono::Duration::hours(hour));
            run_queue_tick_with(db.clone(), &transport, &env).await;
        }
        assert!(notices(&db).is_empty());
        for hour in [25, 26, 30] {
            set_clock(&clock, start + chrono::Duration::hours(hour));
            run_queue_tick_with(db.clone(), &transport, &env).await;
        }
        let raised = notices(&db);
        assert_eq!(raised.len(), 1);
        assert_eq!(raised[0].occurrences, 1, "once, not on every tick");
        assert!(
            raised[0]
                .detail
                .contains("Still waiting after 24 hours. It keeps trying by itself")
        );
        assert_eq!(stored_1601c(&db, &drafts[0]).status, FilingStatus::Queued);
        set_clock(&clock, start + chrono::Duration::hours(49));
        run_queue_tick_with(db.clone(), &transport, &env).await;
        let again = notices(&db);
        assert_eq!(again[0].occurrences, 2);
        assert!(again[0].detail.contains("after 48 hours"));
        transport.set(Far::Up);
        set_clock(&clock, start + chrono::Duration::hours(60));
        run_queue_tick_with(db.clone(), &transport, &env).await;
        assert_eq!(stored_1601c(&db, &drafts[0]).status, FilingStatus::Sent);
        assert!(notices(&db).is_empty(), "sending it clears the notice");
    }

    #[tokio::test]
    async fn a_row_queued_under_the_old_rules_keeps_its_time_once_then_follows_the_new_waits() {
        let (db, drafts) = queued_1601cs("913", &[5]);
        let (clock, env) = sim_clock(11);
        let start = *clock.lock().unwrap();
        // As the old build left it: four tries, the next 8 minutes off, no
        // smart_queue facts.
        let stored = stored_1601c(&db, &drafts[0]);
        let mut old = stored.clone();
        old.submission_attempts = 4;
        old.submission_error = Some(format!("{NOT_SENT_PREFIX}the relay closed early"));
        old.next_retry_at = Some((start + chrono::Duration::minutes(8)).to_rfc3339());
        assert!(
            db.lock()
                .unwrap()
                .replace_unclaimed_queued_1601c_submission(
                    &old,
                    &stored.queued_submission_fingerprint,
                    &stored.next_retry_at,
                    stored.submission_attempts,
                )
                .unwrap()
        );
        let transport = SimTransport::new(Far::UploadFails, &clock);
        run_minutes(&db, &transport, &env, &clock, start, 8).await;
        assert!(transport.tries().is_empty(), "its old time is honoured");
        run_minutes(
            &db,
            &transport,
            &env,
            &clock,
            start + chrono::Duration::minutes(8),
            1,
        )
        .await;
        assert_eq!(transport.tries().len(), 1);
        let after = stored_1601c(&db, &drafts[0]);
        assert_eq!(
            after.status,
            FilingStatus::Queued,
            "a fifth failure no longer drops it to Draft"
        );
        assert_eq!(after.submission_attempts, 5, "history");
        assert_eq!(
            after.smart_queue.failures, 1,
            "the new schedule starts at its first wait"
        );
        let next = stored_next(&db, &drafts[0]).unwrap();
        let tried = start + chrono::Duration::minutes(8);
        assert!(
            next >= tried + chrono::Duration::seconds(150)
                && next <= tried + chrono::Duration::seconds(450),
            "{next}"
        );
    }

    #[tokio::test]
    async fn an_unknown_outcome_is_never_sent_again_by_the_queue() {
        let (db, drafts) = queued_1601cs("914", &[5, 6]);
        let (clock, env) = sim_clock(12);
        let transport = SimTransport::new(Far::Unknown, &clock);
        let start = *clock.lock().unwrap();
        run_minutes(&db, &transport, &env, &clock, start, 2).await;
        transport.set(Far::Up);
        for hours in [1, 5, 30] {
            set_clock(&clock, start + chrono::Duration::hours(hours));
            run_queue_tick_with(db.clone(), &transport, &env).await;
        }
        for draft in &drafts {
            assert_eq!(
                transport.tries_of(&draft.default_submission_filename()),
                1,
                "tried once, never again"
            );
            let row = stored_1601c(&db, draft);
            assert_eq!(row.status, FilingStatus::Queued);
            assert!(
                row.submission_claim_token.is_some() && row.submission_put_started_at.is_some()
            );
        }
    }

    #[tokio::test]
    async fn a_policy_from_the_sign_only_lengthens_the_waits() {
        let (clock, env) = sim_clock(13);
        for (segment, queue, at_least) in [
            (
                "915",
                serde_json::json!({"waits_s": [1800, 3600, 7200, 7200], "repeat_s": 21600}),
                900,
            ),
            (
                "916",
                serde_json::json!({"waits_s": [1, 1, 1, 1], "repeat_s": 60, "spread": 0.99}),
                150,
            ),
        ] {
            let (db, drafts) = queued_1601cs(segment, &[5]);
            let start = Utc::now();
            let transport = SimTransport::new(Far::Outage, &clock);
            let mut object = serde_json::json!({"v": 1, "seq": 1, "state": "no_recent_news"});
            object["queue"] = queue;
            let sign = crate::bir_sign::Sign::from_json(&object).unwrap();
            *transport.sign.lock().unwrap() = Some(SeenSign {
                sign,
                seen_at: start,
            });
            set_clock(&clock, start);
            run_queue_tick_with(db.clone(), &transport, &env).await;
            let next = stored_next(&db, &drafts[0]).unwrap();
            assert!(
                next >= start + chrono::Duration::seconds(at_least),
                "{segment}: {next} is {} s after the try; {:?}",
                (next - start).num_seconds(),
                stored_1601c(&db, &drafts[0]).smart_queue
            );
        }
    }

    /// What an older build sharing the database would find due: it sends
    /// every queued row whose `next_retry_at` has passed, once a minute.
    /// After the new queue's tick, at most one row is due in any minute.
    #[tokio::test]
    async fn an_older_build_on_the_same_database_finds_at_most_one_return_due() {
        // Any seed: the head's new time is spaced from the rows stamped
        // behind it, whatever the random numbers were.
        for seed in 0..40u64 {
            let (db, drafts) = queued_1601cs(&format!("{}", 950 + seed % 40), &[5, 6, 7, 8]);
            let (clock, env) = sim_clock(seed);
            let transport = SimTransport::new(if seed % 2 == 0 { Far::Outage } else { Far::UploadFails }, &clock);
            let start = *clock.lock().unwrap();
            run_queue_tick_with(db.clone(), &transport, &env).await;
            let mut times: Vec<DateTime<Utc>> = drafts
                .iter()
                .map(|draft| stored_next(&db, draft).expect("never null"))
                .collect();
            times.sort();
            for pair in times.windows(2) {
                assert!(
                    pair[1] - pair[0] >= chrono::Duration::seconds(60),
                    "seed {seed}: {pair:?}"
                );
            }
            // An older build ticks once a minute and sends every row whose
            // time has passed: never two in one tick, over the next day.
            for minute in 0..(24 * 60) {
                let tick = start + chrono::Duration::minutes(minute);
                let due = times
                    .iter()
                    .filter(|at| **at <= tick && **at > tick - chrono::Duration::minutes(1))
                    .count();
                assert!(due <= 1, "seed {seed}, minute {minute}");
            }
        }
    }

    /// A stamp is written only when it changes, and is never an edit: an
    /// idle tick rewrites nothing and leaves `updated_at` alone.
    #[tokio::test]
    async fn a_tick_that_changes_nothing_writes_nothing() {
        let (db, _drafts) = queued_1601cs("990", &[5, 6, 7]);
        let (clock, env) = sim_clock(77);
        let transport = SimTransport::new(Far::Outage, &clock);
        let start = *clock.lock().unwrap();
        run_queue_tick_with(db.clone(), &transport, &env).await;
        let snapshot = |db: &Arc<Mutex<Database>>| -> Vec<(i64, String, String)> {
            let guard = db.lock().unwrap();
            let mut rows: Vec<_> = guard
                .list_all_queued_submissions()
                .unwrap()
                .into_iter()
                .map(|s| (s.id, guard.draft_status_and_json(s.id).unwrap().unwrap().1, s.updated_at))
                .collect();
            rows.sort();
            rows
        };
        let before = snapshot(&db);
        for minute in 1..4 {
            set_clock(&clock, start + chrono::Duration::minutes(minute));
            assert_eq!(run_queue_tick_with(db.clone(), &transport, &env).await, None);
        }
        assert_eq!(snapshot(&db), before, "nothing rewritten while the queue waits");
    }

    fn path_setting(db: &Arc<Mutex<Database>>) -> PathBackoff {
        PathBackoff::from_setting(
            db.lock()
                .unwrap()
                .get_setting(smart_queue::PATH_SETTING)
                .unwrap()
                .as_deref(),
        )
    }

    /// Run the worker once a minute for `minutes`, returning the sends.
    async fn probes(
        db: &Arc<Mutex<Database>>,
        transport: &Arc<SimTransport>,
        env: &QueueEnv,
        clock: &Arc<Mutex<DateTime<Utc>>>,
        from: DateTime<Utc>,
        minutes: i64,
    ) -> usize {
        let before = transport.tries().len();
        run_minutes(db, transport, env, clock, from, minutes).await;
        transport.tries().len() - before
    }

    #[tokio::test]
    async fn a_failure_at_the_front_desk_makes_the_whole_queue_wait_and_the_next_probe_is_another_return() {
        let (db, drafts) = queued_1601cs("930", &[5, 6, 7]);
        let (clock, env) = sim_clock(30);
        let transport = SimTransport::new(Far::Outage, &clock);
        let start = *clock.lock().unwrap();
        assert_eq!(
            run_queue_tick_with(db.clone(), &transport, &env).await,
            Some(SendOutcome::Failed(TryClass::Counted))
        );
        let first = transport.tries()[0].0.clone();
        let path = path_setting(&db);
        assert_eq!(path.failures, 1);
        let floor = path.floor(start).expect("a queue-wide wait");
        assert!(
            floor >= start + chrono::Duration::seconds(150) && floor <= start + chrono::Duration::seconds(450),
            "{floor}"
        );
        // The failed return's own count did not move; every queued row is
        // stamped at or after the floor, so an older build waits too.
        for draft in &drafts {
            let row = stored_1601c(&db, draft);
            assert_eq!(row.smart_queue.failures, 0, "the return itself did nothing wrong");
            assert!(stored_next(&db, draft).unwrap() >= floor);
        }
        // Nothing goes before it, however many ticks.
        assert_eq!(
            probes(&db, &transport, &env, &clock, start + chrono::Duration::minutes(1), 2).await,
            0
        );
        // After it exactly one goes, and it is a different return.
        set_clock(&clock, floor + chrono::Duration::seconds(1));
        run_queue_tick_with(db.clone(), &transport, &env).await;
        assert_eq!(transport.tries().len(), 2);
        assert_ne!(transport.tries()[1].0, first, "the probe rotates");
        // The wait grew a step, and it is stored: a restart (a new env, the
        // same database) keeps it.
        let path = path_setting(&db);
        assert_eq!(path.failures, 2);
        let (_, other_env) = sim_clock(31);
        let now = *clock.lock().unwrap();
        assert!(path.floor(now).unwrap() >= now + chrono::Duration::minutes(14));
        assert_eq!(run_queue_tick_with(db.clone(), &transport, &other_env).await, None);
        // A landing anywhere clears it and wakes the queue.
        transport.set(Far::Up);
        set_clock(&clock, path.floor(now).unwrap() + chrono::Duration::seconds(1));
        assert_eq!(
            run_queue_tick_with(db.clone(), &transport, &env).await,
            Some(SendOutcome::Sent)
        );
        assert!(path_setting(&db).is_clear());
    }

    #[tokio::test]
    async fn a_failure_after_the_upload_opened_moves_only_that_return() {
        let (db, drafts) = queued_1601cs("931", &[5, 6, 7]);
        let (clock, env) = sim_clock(32);
        let transport = SimTransport::new(Far::UploadFails, &clock);
        let start = *clock.lock().unwrap();
        run_minutes(&db, &transport, &env, &clock, start, 3).await;
        assert_eq!(transport.tries().len(), 3, "each steps aside for the next");
        assert!(path_setting(&db).is_clear(), "the way in is not blamed");
        for draft in &drafts {
            assert_eq!(stored_1601c(&db, draft).smart_queue.failures, 1);
        }
    }

    #[tokio::test]
    async fn three_refused_logins_in_a_row_stop_that_return_and_nothing_else() {
        let (db, drafts) = queued_1601cs("932", &[5, 6]);
        let (clock, env) = sim_clock(33);
        let transport = SimTransport::new(Far::LoginRefused, &clock);
        let start = *clock.lock().unwrap();
        // May goes first each time; June is never tried while the other
        // return is the due one, so let June wait behind a long clock.
        let mut now = start;
        for _ in 0..3 {
            set_clock(&clock, now);
            let outcome = run_queue_tick_with(db.clone(), &transport, &env).await;
            assert_eq!(outcome, Some(SendOutcome::Failed(TryClass::Counted)));
            now += chrono::Duration::hours(4);
        }
        let tried: Vec<String> = transport.tries().into_iter().map(|(file, _)| file).collect();
        assert_eq!(tried.len(), 3);
        assert!(path_setting(&db).is_clear(), "a login is not the way in");
        // The one tried three times is stopped, with its reason.
        let stuck: Vec<&Form1601CDraft> = drafts
            .iter()
            .filter(|d| stored_1601c(&db, d).smart_queue.is_stuck())
            .collect();
        assert_eq!(stuck.len(), 1);
        let row = stored_1601c(&db, stuck[0]);
        assert_eq!(row.status, FilingStatus::Queued);
        assert!(row.smart_queue.stuck.as_deref().unwrap().contains("login"));
        // No more tries for it, however long we wait; the other return is
        // unaffected and still goes.
        let other = drafts.iter().find(|d| !stored_1601c(&db, d).smart_queue.is_stuck()).unwrap();
        set_clock(&clock, now + chrono::Duration::days(3));
        transport.set(Far::Up);
        assert_eq!(
            run_queue_tick_with(db.clone(), &transport, &env).await,
            Some(SendOutcome::Sent)
        );
        assert_eq!(stored_1601c(&db, other).status, FilingStatus::Sent);
        assert_eq!(run_queue_tick_with(db.clone(), &transport, &env).await, None);
        let tries = transport.tries().len();
        set_clock(&clock, now + chrono::Duration::days(30));
        assert_eq!(run_queue_tick_with(db.clone(), &transport, &env).await, None);
        assert_eq!(transport.tries().len(), tries, "stopped until the person presses Retry");
        // The person's Retry ends the stop: it goes.
        let mut state = row.smart_queue.clone();
        state.press_retry(now + chrono::Duration::days(30), false);
        let mut pressed = row.clone();
        pressed.smart_queue = state;
        pressed.next_retry_at = Some((now + chrono::Duration::days(30)).to_rfc3339());
        assert!(
            db.lock()
                .unwrap()
                .replace_unclaimed_queued_1601c_submission(
                    &pressed,
                    &row.queued_submission_fingerprint,
                    &row.next_retry_at,
                    row.submission_attempts,
                )
                .unwrap()
        );
        assert_eq!(
            run_queue_tick_with(db.clone(), &transport, &env).await,
            Some(SendOutcome::Sent)
        );
    }

    /// A session the server keeps refusing with a 4xx (a revoked device
    /// key): not a try, so no count, no path back-off and no device guard;
    /// but the same answer three times in a row stops that return with the
    /// server's sentence, until the person presses Retry.
    #[tokio::test]
    async fn a_session_the_server_keeps_refusing_stops_the_return_and_retry_lifts_it() {
        let (db, drafts) = queued_1601cs("936", &[5]);
        let (clock, env) = sim_clock(37);
        let transport = SimTransport::new(Far::SessionRefused, &clock);
        let mut now = *clock.lock().unwrap();
        for tick in 1..=3 {
            set_clock(&clock, now);
            assert_eq!(
                run_queue_tick_with(db.clone(), &transport, &env).await,
                Some(SendOutcome::Failed(TryClass::Answered))
            );
            let row = stored_1601c(&db, &drafts[0]);
            assert_eq!(row.smart_queue.is_stuck(), tick == 3, "after {tick}");
            now += chrono::Duration::hours(4);
        }
        let row = stored_1601c(&db, &drafts[0]);
        assert_eq!(row.status, FilingStatus::Queued);
        assert_eq!(
            (row.submission_attempts, row.smart_queue.failures),
            (0, 0),
            "not a try"
        );
        assert!(row.smart_queue.stuck.as_deref().unwrap().contains("(device_not_allowed)"));
        assert!(row.smart_queue.stuck.as_deref().unwrap().contains("This device may not send returns."));
        assert!(path_setting(&db).is_clear(), "the way in is not blamed");
        assert_eq!(device_tries(&db).to_setting(), "", "not the device's connection");
        assert!(transport.tries().is_empty(), "nothing was sent");
        // Stopped, however long we wait.
        set_clock(&clock, now + chrono::Duration::days(30));
        assert_eq!(run_queue_tick_with(db.clone(), &transport, &env).await, None);
        // The person's Retry lifts it; the session works now.
        transport.set(Far::Up);
        let mut state = row.smart_queue.clone();
        state.press_retry(now + chrono::Duration::days(30), false);
        let mut pressed = row.clone();
        pressed.smart_queue = state;
        pressed.next_retry_at = Some((now + chrono::Duration::days(30)).to_rfc3339());
        assert!(
            db.lock()
                .unwrap()
                .replace_unclaimed_queued_1601c_submission(
                    &pressed,
                    &row.queued_submission_fingerprint,
                    &row.next_retry_at,
                    row.submission_attempts,
                )
                .unwrap()
        );
        assert_eq!(
            run_queue_tick_with(db.clone(), &transport, &env).await,
            Some(SendOutcome::Sent)
        );
    }

    #[tokio::test]
    async fn no_upload_service_for_a_tin_stops_after_three_and_is_not_the_device() {
        let (db, drafts) = queued_1601cs("933", &[5]);
        let (clock, env) = sim_clock(34);
        let transport = SimTransport::new(Far::NoService, &clock);
        let mut now = *clock.lock().unwrap();
        for _ in 0..3 {
            set_clock(&clock, now);
            run_queue_tick_with(db.clone(), &transport, &env).await;
            now += chrono::Duration::hours(4);
        }
        let row = stored_1601c(&db, &drafts[0]);
        assert!(row.smart_queue.is_stuck(), "{:?}", row.smart_queue);
        assert_eq!(device_tries(&db).to_setting(), "", "not the device's connection");
        assert!(path_setting(&db).is_clear());
    }

    /// Class `ours` with nothing opened is not a try, on a ladder of its own
    /// (5 min, 30 min, 1 h, 2 h, then 2 h); a stream that opened is counted
    /// whatever the class.
    #[tokio::test]
    async fn class_ours_is_not_a_try_only_when_no_stream_opened_and_never_loops_short() {
        let (db, drafts) = queued_1601cs("934", &[5]);
        let (clock, env) = sim_clock(35);
        let transport = SimTransport::new(Far::OursNothingOpened, &clock);
        let mut now = *clock.lock().unwrap();
        let mut waits = Vec::new();
        for _ in 0..6 {
            set_clock(&clock, now);
            assert_eq!(
                run_queue_tick_with(db.clone(), &transport, &env).await,
                Some(SendOutcome::Failed(TryClass::NotATry))
            );
            let row = stored_1601c(&db, &drafts[0]);
            assert_eq!((row.submission_attempts, row.smart_queue.failures), (0, 0));
            let next = stored_next(&db, &drafts[0]).unwrap();
            waits.push((next - now).num_seconds());
            now = next;
        }
        for (wait, base) in waits.iter().zip([300, 1800, 3600, 7200, 7200, 7200]) {
            assert!(*wait as f64 >= base as f64 * 0.5 - 1.0 && *wait as f64 <= base as f64 * 1.5 + 1.0, "{waits:?}");
        }
        assert_eq!(stored_1601c(&db, &drafts[0]).smart_queue.not_tried, 6);
        assert!(path_setting(&db).is_clear());
        // The same class, but a stream opened: counted, and it resets the
        // not-a-try count.
        transport.set(Far::OursAfterOpening);
        set_clock(&clock, now);
        assert_eq!(
            run_queue_tick_with(db.clone(), &transport, &env).await,
            Some(SendOutcome::Failed(TryClass::Counted))
        );
        let row = stored_1601c(&db, &drafts[0]);
        assert_eq!((row.submission_attempts, row.smart_queue.failures, row.smart_queue.not_tried), (1, 1, 0));
    }

    /// A send that never answers is cut at its deadline: "BIR may have it",
    /// never sent again by itself, and the queue goes on with the next.
    #[tokio::test]
    async fn a_hung_send_is_maybe_sent_never_resent_and_does_not_stop_the_queue() {
        let (db, drafts) = queued_1601cs("935", &[5, 6]);
        let (clock, env) = sim_clock(36);
        let env = env.with_deadlines(
            std::time::Duration::from_millis(60),
            std::time::Duration::from_millis(60),
        );
        let transport = SimTransport::new(Far::Hang, &clock);
        let start = *clock.lock().unwrap();
        assert_eq!(
            run_queue_tick_with(db.clone(), &transport, &env).await,
            Some(SendOutcome::Failed(TryClass::MaybeSent))
        );
        let hung = transport.tries()[0].0.clone();
        let hung_draft = drafts.iter().find(|d| d.default_submission_filename() == hung).unwrap();
        let row = stored_1601c(&db, hung_draft);
        assert!(row.submission_claim_token.is_some(), "claimed, PUT started: needs attention");
        assert!(row.submission_put_started_at.is_some());
        assert!(row.submission_error.as_deref().unwrap().contains("outcome pending"), "{:?}", row.submission_error);
        // The queue goes on: the other return goes next, and the hung one
        // is never touched again, however long we wait.
        transport.set(Far::Up);
        set_clock(&clock, start + chrono::Duration::minutes(1));
        assert_eq!(
            run_queue_tick_with(db.clone(), &transport, &env).await,
            Some(SendOutcome::Sent)
        );
        assert_ne!(transport.tries()[1].0, hung);
        set_clock(&clock, start + chrono::Duration::days(10));
        assert_eq!(run_queue_tick_with(db.clone(), &transport, &env).await, None);
        assert_eq!(transport.tries_of(&hung), 1);
        // (The hung send never decremented the counter: its future was dropped.)
    }

    /// Shutdown cuts a tick that waits on the network. The send it cut is
    /// claimed with its PUT started, so a restart never sends it again.
    #[tokio::test]
    async fn shutdown_interrupts_a_tick_and_a_restart_does_not_resend_that_return() {
        let (db, drafts) = queued_1601cs("936", &[5]);
        let (clock, env) = sim_clock(37);
        let transport = SimTransport::new(Far::Hang, &clock);
        let stop = Notify::new();
        let tick = run_tick_until(&stop, db.clone(), &transport, &env);
        let ring = async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            stop.notify_one();
        };
        let (cut, ()) = tokio::join!(tick, ring);
        assert!(cut.is_none(), "the tick was cut short");
        let row = stored_1601c(&db, &drafts[0]);
        assert!(row.submission_claim_token.is_some() && row.submission_put_started_at.is_some());
        // A new process, the same database: the claim is held, nothing goes.
        let (clock2, env2) = sim_clock(38);
        let again = SimTransport::new(Far::Up, &clock2);
        set_clock(&clock2, *clock.lock().unwrap() + chrono::Duration::days(2));
        assert_eq!(run_queue_tick_with(db.clone(), &again, &env2).await, None);
        assert!(again.tries().is_empty());
    }

    /// A head the worker cannot take (its clock reads earlier than the time
    /// the line stamped) is not tried: it waits on a growing ladder of its
    /// own and the next return goes in the same tick.
    #[tokio::test]
    async fn a_head_that_is_not_tried_waits_longer_each_time_and_the_next_goes() {
        let (db, drafts) = queued_1601cs("937", &[5, 6]);
        let t0 = Utc::now();
        // The tick reads the clock first; the worker's first read (the
        // second call) is an hour behind, so the head is not yet due to it.
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = calls.clone();
        let env = QueueEnv::new(
            move || {
                if counted.fetch_add(1, Ordering::SeqCst) == 1 {
                    t0 - chrono::Duration::hours(1)
                } else {
                    t0
                }
            },
            SeededRng::new(39),
        );
        let clock = Arc::new(Mutex::new(t0));
        let transport = SimTransport::new(Far::Up, &clock);
        assert_eq!(
            run_queue_tick_with(db.clone(), &transport, &env).await,
            Some(SendOutcome::Sent),
            "the next return goes in the same tick"
        );
        assert_eq!(transport.tries().len(), 1);
        assert_eq!(transport.tries()[0].0, drafts[1].default_submission_filename());
        let may = stored_1601c(&db, &drafts[0]);
        assert_eq!(may.status, FilingStatus::Queued);
        assert_eq!(may.smart_queue.skips, 1);
        assert_eq!(may.smart_queue.failures, 0, "not a failure of the return");
        let wait = (stored_next(&db, &drafts[0]).unwrap() - t0).num_seconds();
        assert!((150..=450).contains(&wait), "{wait} s");
    }

    /// A time that cannot be read never sends a return to Draft: the tick
    /// stamps a real one.
    #[tokio::test]
    async fn an_unreadable_retry_time_is_restamped_and_never_sends_the_return_to_draft() {
        let (db, drafts) = queued_1601cs("938", &[5]);
        let (clock, env) = sim_clock(40);
        let transport = SimTransport::new(Far::Up, &clock);
        let stored = stored_1601c(&db, &drafts[0]);
        let mut bad = stored.clone();
        bad.next_retry_at = Some("not a time".into());
        assert!(
            db.lock()
                .unwrap()
                .replace_unclaimed_queued_1601c_submission(
                    &bad,
                    &stored.queued_submission_fingerprint,
                    &stored.next_retry_at,
                    stored.submission_attempts,
                )
                .unwrap()
        );
        let outcome = run_queue_tick_with(db.clone(), &transport, &env).await;
        assert_ne!(stored_1601c(&db, &drafts[0]).status, FilingStatus::Draft);
        assert!(matches!(outcome, Some(SendOutcome::Sent)), "{outcome:?}");
    }

    /// A clock set back a day: the waits the queue stored are kept from the
    /// time the try ended, so nobody is stranded and nobody goes early.
    #[tokio::test]
    async fn a_clock_set_back_keeps_the_same_wait_from_now_and_never_goes_early() {
        let (db, drafts) = queued_1601cs("939", &[5]);
        let (clock, env) = sim_clock(41);
        let transport = SimTransport::new(Far::UploadFails, &clock);
        let start = *clock.lock().unwrap();
        run_queue_tick_with(db.clone(), &transport, &env).await;
        let wait = stored_next(&db, &drafts[0]).unwrap() - start;
        let back = start - chrono::Duration::days(1);
        // The clock reads a day earlier: the wait owed is counted from here.
        set_clock(&clock, back);
        assert_eq!(run_queue_tick_with(db.clone(), &transport, &env).await, None);
        set_clock(&clock, back + wait - chrono::Duration::seconds(30));
        assert_eq!(run_queue_tick_with(db.clone(), &transport, &env).await, None, "not before the same wait");
        set_clock(&clock, back + wait + chrono::Duration::seconds(90));
        assert!(run_queue_tick_with(db.clone(), &transport, &env).await.is_some());
    }

    /// "Still waiting": cleared when the return is cancelled or refused, and
    /// not raised again after an older build rewrote the row.
    #[tokio::test]
    async fn the_still_waiting_notice_is_cleared_when_the_return_leaves_and_not_repeated() {
        let (db, drafts) = queued_1601cs("940", &[5]);
        let (clock, env) = sim_clock(42);
        let transport = SimTransport::new(Far::Outage, &clock);
        let start = *clock.lock().unwrap();
        let notices = |db: &Arc<Mutex<Database>>| {
            db.lock()
                .unwrap()
                .list_active_alerts(None)
                .unwrap()
                .into_iter()
                .filter(|alert| alert.kind.starts_with("queue_still_waiting:"))
                .count()
        };
        set_clock(&clock, start + chrono::Duration::hours(25));
        run_queue_tick_with(db.clone(), &transport, &env).await;
        assert_eq!(notices(&db), 1);
        // An older build rewrote the row without the queue's facts.
        let row = stored_1601c(&db, &drafts[0]);
        let mut old = row.clone();
        old.smart_queue = QueueState::default();
        assert!(
            db.lock()
                .unwrap()
                .replace_unclaimed_queued_1601c_submission(
                    &old,
                    &row.queued_submission_fingerprint,
                    &row.next_retry_at,
                    row.submission_attempts,
                )
                .unwrap()
        );
        set_clock(&clock, start + chrono::Duration::hours(26));
        run_queue_tick_with(db.clone(), &transport, &env).await;
        let raised = db.lock().unwrap().list_active_alerts(None).unwrap();
        let occurrences: u32 = raised
            .iter()
            .filter(|alert| alert.kind.starts_with("queue_still_waiting:"))
            .map(|alert| alert.occurrences as u32)
            .sum();
        assert_eq!(occurrences, 1, "not raised again");
        // Cancelled: the notice goes.
        let current = stored_1601c(&db, &drafts[0]);
        db.lock().unwrap().cancel_queued_1601c_submission(&current).unwrap();
        set_clock(&clock, start + chrono::Duration::hours(27));
        run_queue_tick_with(db.clone(), &transport, &env).await;
        assert_eq!(notices(&db), 0);
    }

    /// A database holding one saved profile (TIN segment `segment3`) and
    /// `n` queued 1601Cs (up to 24: the months of 2023 and 2024).
    fn queued_n_1601cs(segment3: &str, n: usize) -> (Arc<Mutex<Database>>, Vec<Form1601CDraft>) {
        let db = test_db();
        let profile = db
            .lock()
            .unwrap()
            .save_profile(reviewed_profile_with_tin(segment3))
            .expect("the profile should persist");
        let drafts = (0..n)
            .map(|i| {
                let (year, month) = (2023 + (i / 12) as u16, (i % 12) as u8 + 1);
                let mut draft = Form1601CDraft::new_from_profile(&profile, year, month);
                draft.any_taxes_withheld = false;
                draft.transition_to_queued().expect("the 1601C should queue");
                db.lock().unwrap().save_queued_1601c_draft(&draft).expect("persist");
                draft
            })
            .collect();
        (db, drafts)
    }

    /// What one simulated outage came to.
    struct Outage {
        /// Every try, any return: how many.
        attempts: usize,
        /// The most sends at once.
        most_at_once: usize,
        /// Minutes from the BIR coming back until the last return is sent
        /// (`None` when some was not sent by the end).
        sent_after: Option<i64>,
        /// Returns still queued at the end.
        still_queued: usize,
        /// Tries made in the first 20 minutes.
        first_20_minutes: usize,
    }

    /// `n` queued returns, the BIR down for `outage_minutes` (every try
    /// fails at the front desk), then up for `after_minutes`; the worker
    /// ticks once a minute, as it does.
    async fn simulate_outage(segment3: &str, n: usize, seed: u64, outage_minutes: i64, after_minutes: i64) -> Outage {
        let (db, drafts) = queued_n_1601cs(segment3, n);
        let (clock, env) = sim_clock(seed);
        let transport = SimTransport::new(Far::Outage, &clock);
        let start = *clock.lock().unwrap();
        let mut done_at = None;
        for minute in 0..outage_minutes + after_minutes {
            if minute == outage_minutes {
                transport.set(Far::Up);
            }
            set_clock(&clock, start + chrono::Duration::minutes(minute));
            run_queue_tick_with(db.clone(), &transport, &env).await;
            if done_at.is_none()
                && minute >= outage_minutes
                && drafts.iter().all(|d| stored_1601c(&db, d).status == FilingStatus::Sent)
            {
                done_at = Some(minute - outage_minutes);
            }
        }
        let tries = transport.tries();
        let first = tries[0].1;
        Outage {
            attempts: tries.len(),
            most_at_once: transport.most_in_flight(),
            sent_after: done_at,
            still_queued: drafts.iter().filter(|d| stored_1601c(&db, d).status == FilingStatus::Queued).count(),
            first_20_minutes: tries.iter().filter(|(_, at)| *at < first + chrono::Duration::minutes(20)).count(),
        }
    }

    /// An 18-hour outage costs the same number of tries whether 1, 3 or 20
    /// returns are queued: the whole queue waits as one on the ladder
    /// (5 min, 30 min, 1 h, 2 h, then 2 h, each x 0.5 to 1.5), one probe at
    /// a time, and nothing is dropped. The old queue made 5 tries per
    /// return at 0, 1, 3, 7 and 15 minutes (5, 15 and 100 tries for 1, 3
    /// and 20 returns, all at once) and then sent every return to Draft.
    #[tokio::test]
    async fn an_18_hour_outage_costs_the_same_tries_for_1_3_and_20_returns() {
        for (segment, seed) in [("918", 18), ("919", 1918), ("920", 2026)] {
            let one = simulate_outage(segment, 1, seed, 18 * 60, 0).await;
            let three = simulate_outage(&format!("{segment}1"), 3, seed, 18 * 60, 0).await;
            let twenty = simulate_outage(&format!("{segment}2"), 20, seed, 18 * 60, 0).await;
            eprintln!(
                "18 h outage, seed {seed}: attempts 1/3/20 returns = {}/{}/{}, most at once {}/{}/{}, first 20 min {}/{}/{}",
                one.attempts, three.attempts, twenty.attempts,
                one.most_at_once, three.most_at_once, twenty.most_at_once,
                one.first_20_minutes, three.first_20_minutes, twenty.first_20_minutes,
            );
            for outage in [&one, &three, &twenty] {
                assert_eq!(outage.most_at_once, 1, "never two sends at once");
                // 1 + 4 steps up the ladder + the 2-hour repeats, over
                // 18 hours, from every wait at x 1.5 (9 tries) to x 0.5 (21).
                assert!((9..=21).contains(&outage.attempts), "{} attempts", outage.attempts);
                assert!(outage.first_20_minutes <= 3, "{} tries in the first 20 minutes", outage.first_20_minutes);
            }
            // The same random numbers, the same ladder: 3 and 20 returns
            // cost the same as 1, within the spread of one step.
            for many in [&three, &twenty] {
                assert!(many.attempts.abs_diff(one.attempts) <= 1, "{} vs {}", many.attempts, one.attempts);
            }
            assert_eq!(one.still_queued, 1);
            assert_eq!(three.still_queued, 3);
            assert_eq!(twenty.still_queued, 20, "nothing was sent back to Draft");
        }
    }

    /// A 1-hour outage: the queue probes on the ladder, and once the BIR is
    /// back the first probe that lands wakes the queue: every return goes,
    /// one a tick. Numbers: all sent within the ladder's next wait (at most
    /// 1.5 x 1 h after the 35-minute try) plus one minute per return.
    #[tokio::test]
    async fn a_one_hour_outage_sends_every_return_soon_after_it_ends() {
        for (segment, seed) in [("921", 1), ("922", 60), ("923", 3600)] {
            for n in [1usize, 3, 20] {
                let outage = simulate_outage(&format!("{segment}{n}"), n, seed, 60, 4 * 60).await;
                let sent_after = outage.sent_after.expect("every return was sent");
                eprintln!(
                    "1 h outage, seed {seed}, {n} returns: {} attempts, all sent {sent_after} min after the BIR came back, most at once {}",
                    outage.attempts, outage.most_at_once
                );
                assert_eq!(outage.most_at_once, 1);
                assert_eq!(outage.still_queued, 0);
                // Failed probes before it ended (at most 4), then n sends.
                assert!(outage.attempts > n && outage.attempts <= n + 5, "{}", outage.attempts);
                assert!(sent_after <= 90 + n as i64 + 2, "{sent_after} min");
            }
        }
    }
}
