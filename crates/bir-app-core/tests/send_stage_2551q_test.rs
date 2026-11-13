//! A 2551Q keeps its send reasons in `last_error`: the stages the app shows
//! read them there, as the queue worker leaves them.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use bir_app_core::bir_core::background_cron::{StoreFailure, StoreReport, SubmissionTransport, TransportError, get_active_jobs, run_queue_tick};
use bir_app_core::bir_core::db::{Claim2551QSubmissionResult, Database};
use bir_app_core::{BirApp, BranchFacts, NewTaxpayer, OpenReturn, SendStage};
use serde_json::Value;

const TEST_KEY: &str = "0000000000000000000000000000000000000000000000000000000000000000";
const YEAR: u16 = 2026;

/// A relay that opens and then says nothing reached BIR.
struct NotSentTransport;

impl SubmissionTransport for NotSentTransport {
    type Session = ();

    fn open_session<'a>(&'a self, _form_type: &'a str, _tin: &'a str) -> Pin<Box<dyn Future<Output = Result<(), TransportError>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }

    fn store_session<'a>(&'a self, _session: (), _filename: &'a str, _payload: &'a [u8]) -> Pin<Box<dyn Future<Output = Result<(), TransportError>> + Send + 'a>> {
        Box::pin(async { Err(TransportError("the relay refused the ticket".into())) })
    }

    fn store_session_reported<'a>(&'a self, _session: (), _filename: &'a str, _payload: &'a [u8]) -> Pin<Box<dyn Future<Output = Result<StoreReport, StoreFailure>> + Send + 'a>> {
        Box::pin(async { Err(StoreFailure::NotSent(TransportError("the relay refused the ticket".into()))) })
    }
}

/// A database with the demo taxpayer and a queued Q1 2551Q: the app, the
/// worker's connection, the TIN and the return id.
fn queued_2551q(dir: &std::path::Path) -> (BirApp, Arc<Mutex<Database>>, String, i64) {
    let path = dir.join("bir_data.db");
    let app = BirApp::open_at(&path).expect("open");
    let tp = app
        .create_taxpayer(&NewTaxpayer {
            tin_root: "000123453".into(),
            name: "Juana Santos".into(),
            taxpayer_type: "Individual".into(),
            birth_date: Some("1990-01-15".into()),
            head_office: BranchFacts {
                rdo_code: "043".into(),
                registered_address: "1 Sample Street, Pasig City".into(),
                zip_code: "1600".into(),
                phone: "09171234567".into(),
                email: "juana@example.com".into(),
            },
        })
        .expect("taxpayer");
    let tin = tp.branches[0].tin.clone();
    let opened = app.open_return(&OpenReturn { tin: tin.clone(), form_code: "2551Q".into(), year: YEAR.into(), period: 1, key: None }).expect("open 2551Q");
    let mut draft = opened.draft;
    draft.0["item_13_election"] = "graduated".into();
    let checked = app.validate("2551Q", &draft).expect("validate");
    assert!(checked.errors.is_empty(), "{:?}", checked.errors);
    let saved = app.save("2551Q", &checked.draft).expect("save");
    let id = app.queue_return("2551Q", &saved.draft.unwrap_or(checked.draft)).expect("queue");
    let worker = Arc::new(Mutex::new(Database::open(&path).expect("the worker's connection")));
    (app, worker, tin, id)
}

fn row(worker: &Arc<Mutex<Database>>, id: i64) -> (String, Value) {
    let (status, json) = worker.lock().unwrap().draft_status_and_json(id).unwrap().expect("stored");
    (status, serde_json::from_str(&json).unwrap())
}

/// One queue tick, then wait for its spawned 2551Q task to finish: its job
/// key stays in the in-flight set until it returns, and it is the only job
/// in this test, so the next tick is never skipped as "already running".
async fn tick(worker: &Arc<Mutex<Database>>, id: i64) {
    let before = row(worker, id).1["submission_attempts"].clone();
    run_queue_tick(worker.clone(), &Arc::new(NotSentTransport)).await;
    for _ in 0..500 {
        if get_active_jobs().lock().unwrap().is_empty() {
            assert_ne!(row(worker, id).1["submission_attempts"], before, "the attempt is recorded");
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("the 2551Q queue task did not finish");
}

/// Move the stored retry time into the past, as the cron tests do.
fn retry_due(worker: &Arc<Mutex<Database>>, id: i64) {
    let db = worker.lock().unwrap();
    let (status, json) = db.draft_status_and_json(id).unwrap().unwrap();
    let mut data: Value = serde_json::from_str(&json).unwrap();
    data["next_retry_at"] = "1999-01-01T00:00:00Z".into();
    assert!(db.replace_draft_json_if_unchanged(id, &json, &status, &data.to_string()).unwrap());
}

#[test]
fn a_2551q_send_reason_is_shown() {
    // SAFETY: set once, before any database opens.
    unsafe { std::env::set_var("BUWIZ_DB_KEY", TEST_KEY) };
    a_not_sent_2551q_reads_failed_then_gave_up();
    a_claimed_2551q_reads_needs_attention();
}

fn a_not_sent_2551q_reads_failed_then_gave_up() {
    let dir = tempfile::tempdir().unwrap();
    let (app, worker, _tin, id) = queued_2551q(dir.path());
    let runtime = tokio::runtime::Runtime::new().unwrap();

    runtime.block_on(tick(&worker, id));
    match app.send_states().unwrap().get(&id) {
        Some(SendStage::Failed { reason, attempts: 1, next_retry_at: Some(_) }) => {
            assert!(reason.starts_with("Not sent (nothing reached BIR): "), "{reason}");
        }
        other => panic!("expected Failed after a not-sent 2551Q, got {other:?}"),
    }

    for _ in 2..=5 {
        retry_due(&worker, id);
        runtime.block_on(tick(&worker, id));
    }
    assert_eq!(row(&worker, id).0, "Draft");
    match app.send_states().unwrap().get(&id) {
        Some(SendStage::GaveUp { reason }) => assert!(reason.starts_with("Not sent (nothing reached BIR): "), "{reason}"),
        other => panic!("expected GaveUp after the fifth not-sent 2551Q, got {other:?}"),
    }
}

fn a_claimed_2551q_reads_needs_attention() {
    let dir = tempfile::tempdir().unwrap();
    let (app, worker, tin, id) = queued_2551q(dir.path());
    {
        let db = worker.lock().unwrap();
        let queued = db.get_2551q_draft(&tin, YEAR, 1).unwrap().unwrap();
        let Claim2551QSubmissionResult::Claimed { token, .. } = db
            .claim_queued_2551q_submission(&tin, YEAR, 1, &queued.queued_submission_fingerprint, &queued.next_retry_at, queued.submission_attempts)
            .unwrap()
        else {
            panic!("the 2551Q should claim");
        };
        assert!(db.mark_claimed_2551q_put_started(&tin, YEAR, 1, &token).unwrap());
    }
    let pending = row(&worker, id).1["last_error"].as_str().expect("the claim's note").to_owned();
    match app.send_states().unwrap().get(&id) {
        Some(SendStage::NeedsAttention { reason }) => assert_eq!(reason, &pending),
        other => panic!("expected NeedsAttention for a claimed 2551Q, got {other:?}"),
    }
}
