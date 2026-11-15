//! A person's Retry is limited, and a cancel then a requeue gets no earlier
//! try than the schedule gave.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use bir_app_core::bir_core::background_cron::{StoreFailure, StoreReport, SubmissionTransport, TransportError};
use bir_app_core::bir_core::db::Database;
use bir_app_core::bir_core::smart_queue::{PATH_SETTING, PathBackoff};
use bir_app_core::{BirApp, BranchFacts, NewTaxpayer, OpenReturn, RetryAnswer, SendStage};
use chrono::{Duration, Utc};
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


/// Put the queue's own facts on the queued return, as failed tries leave them.
fn failed_tries(worker: &Arc<Mutex<Database>>, id: i64, failures: u32, next: chrono::DateTime<Utc>) {
    let db = worker.lock().unwrap();
    let (status, json) = db.draft_status_and_json(id).unwrap().unwrap();
    let mut data: Value = serde_json::from_str(&json).unwrap();
    data["smart_queue"] = serde_json::json!({
        "failures": failures,
        "next_try_at": next.to_rfc3339(),
        "ended_at": (next - Duration::minutes(30)).to_rfc3339(),
    });
    data["next_retry_at"] = next.to_rfc3339().into();
    data["last_error"] = "Not sent (nothing reached BIR): the relay closed early".into();
    assert!(db.replace_draft_json_if_unchanged(id, &json, &status, &data.to_string()).unwrap());
}

#[test]
fn retry_is_limited_cancel_and_requeue_keep_the_schedule() {
    // SAFETY: set once, before any database opens.
    unsafe { std::env::set_var("BUWIZ_DB_KEY", TEST_KEY) };
    retry_is_once_per_30_minutes_and_three_times_a_day();
    retry_waits_for_the_path_back_off_unless_the_person_says_try_anyway();
    a_cancel_and_requeue_loop_gets_no_earlier_try();
    a_stopped_return_reads_needs_you_and_retry_lifts_it();
}

fn retry_is_once_per_30_minutes_and_three_times_a_day() {
    let dir = tempfile::tempdir().unwrap();
    let (app, worker, _tin, id) = queued_2551q(dir.path());
    failed_tries(&worker, id, 2, Utc::now() + Duration::hours(1));
    let t0 = Utc::now();
    assert!(matches!(app.retry_at(id, false, t0).unwrap(), RetryAnswer::Going { not_before: None }));
    // Pressed again at once: no.
    let again = t0 + Duration::minutes(5);
    assert_eq!(app.retry_at(id, false, again).unwrap(), RetryAnswer::NotBefore(t0 + Duration::minutes(30)));
    // Nothing changed by the refused press.
    assert!(app.retry_now(id).is_err(), "the plain call says when it may be pressed again");
    assert!(matches!(app.retry_at(id, false, t0 + Duration::minutes(31)).unwrap(), RetryAnswer::Going { .. }));
    assert!(matches!(app.retry_at(id, false, t0 + Duration::minutes(62)).unwrap(), RetryAnswer::Going { .. }));
    // Three in the day: the fourth waits until the first is a day old.
    assert_eq!(
        app.retry_at(id, false, t0 + Duration::minutes(100)).unwrap(),
        RetryAnswer::NotBefore(t0 + Duration::days(1))
    );
    assert!(matches!(app.retry_at(id, false, t0 + Duration::days(1) + Duration::minutes(1)).unwrap(), RetryAnswer::Going { .. }));
    // Its failures stay: Retry never resets the ladder.
    assert_eq!(row(&worker, id).1["smart_queue"]["failures"], 2);
}

fn retry_waits_for_the_path_back_off_unless_the_person_says_try_anyway() {
    let dir = tempfile::tempdir().unwrap();
    let (app, worker, _tin, id) = queued_2551q(dir.path());
    let floor = Utc::now() + Duration::hours(1);
    let path = PathBackoff {
        failures: 2,
        next_try_at: Some(floor.to_rfc3339()),
        ended_at: Some(Utc::now().to_rfc3339()),
        ..PathBackoff::default()
    };
    worker.lock().unwrap().set_setting(PATH_SETTING, &path.to_setting()).unwrap();
    let now = Utc::now();
    match app.retry_at(id, false, now).unwrap() {
        RetryAnswer::Going { not_before: Some(at) } => assert_eq!(at.timestamp(), floor.timestamp()),
        other => panic!("{other:?}"),
    }
    assert_eq!(row(&worker, id).1["smart_queue"]["try_anyway"], Value::Null, "not set");
    let (app2, worker2, _tin2, id2) = {
        let dir2 = tempfile::tempdir().unwrap();
        let made = queued_2551q(dir2.path());
        std::mem::forget(dir2);
        made
    };
    worker2.lock().unwrap().set_setting(PATH_SETTING, &path.to_setting()).unwrap();
    assert_eq!(app2.retry_at(id2, true, now).unwrap(), RetryAnswer::Going { not_before: None });
    assert_eq!(row(&worker2, id2).1["smart_queue"]["try_anyway"], true);
}

fn a_cancel_and_requeue_loop_gets_no_earlier_try() {
    use bir_app_core::bir_core::background_cron::{get_active_jobs, run_queue_tick};
    let dir = tempfile::tempdir().unwrap();
    let (app, worker, tin, id) = queued_2551q(dir.path());
    // A real failed try leaves the queue's facts on the return.
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        run_queue_tick(worker.clone(), &Arc::new(NotSentTransport)).await;
        for _ in 0..500 {
            if get_active_jobs().lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    });
    let (_, data) = row(&worker, id);
    assert_eq!(data["smart_queue"]["failures"], 1);
    let next = chrono::DateTime::parse_from_rfc3339(data["next_retry_at"].as_str().unwrap()).unwrap();
    assert!(next.timestamp() > Utc::now().timestamp() + 100, "a wait is owed");
    let open = || OpenReturn { tin: tin.clone(), form_code: "2551Q".into(), year: YEAR.into(), period: 1, key: None };
    for _ in 0..3 {
        let opened = app.open_return(&open()).expect("open");
        app.cancel_queued("2551Q", &opened.draft).expect("cancel");
        let reopened = app.open_return(&open()).expect("open");
        let id_again = app.queue_return("2551Q", &reopened.draft).expect("queue again");
        assert_eq!(id_again, id);
        let (status, data) = row(&worker, id);
        assert_eq!(status, "Queued");
        assert_eq!(data["smart_queue"]["failures"], 1, "the failures stay");
        let stored = chrono::DateTime::parse_from_rfc3339(data["next_retry_at"].as_str().unwrap()).unwrap();
        assert!(stored.timestamp() >= next.timestamp(), "no earlier try: {stored} < {next}");
    }
}

fn a_stopped_return_reads_needs_you_and_retry_lifts_it() {
    let dir = tempfile::tempdir().unwrap();
    let (app, worker, _tin, id) = queued_2551q(dir.path());
    {
        let db = worker.lock().unwrap();
        let (status, json) = db.draft_status_and_json(id).unwrap().unwrap();
        let mut data: Value = serde_json::from_str(&json).unwrap();
        data["smart_queue"] = serde_json::json!({
            "failures": 3, "kind": "the login was refused", "same_kind": 3, "stuck": "the login was refused"
        });
        data["last_error"] = "Not sent (nothing reached BIR): the login was refused".into();
        assert!(db.replace_draft_json_if_unchanged(id, &json, &status, &data.to_string()).unwrap());
    }
    match app.send_states().unwrap().get(&id) {
        Some(SendStage::NeedsYou { reason }) => assert!(reason.contains("login")),
        other => panic!("{other:?}"),
    }
    assert!(matches!(app.retry(id, false).unwrap(), RetryAnswer::Going { .. }));
    assert!(row(&worker, id).1["smart_queue"].get("stuck").is_none(), "Retry lifts the stop");
}
