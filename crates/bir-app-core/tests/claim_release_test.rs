//! A queued return the worker left claimed (the send outcome was unknown)
//! goes back to Draft only when a person confirms nothing reached BIR:
//! `release_abandoned_claim` with the library's reason, never for a sent
//! or an unclaimed return, never for a claim younger than the claim lease,
//! never while a send for it may be running (this process's worker has its
//! job, or another process runs the queue), and only for the claim the
//! person looked at. `queued_send_state` tells a screen it is stuck and
//! whether it can be released.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use bir_app_core::bir_core::background_cron::{StoreFailure, StoreReport, SubmissionTransport, TransportError, get_active_jobs, queue_job_key, run_queue_tick};
use bir_app_core::bir_core::db::{Claim1601CSubmissionResult, Claim2551QSubmissionResult, ClaimQueueableResult, Database, WorkerLease, worker_lock_path};
use bir_app_core::bir_core::forms::form_0619e::Form0619EDraft;
use bir_app_core::bir_core::forms::queueable::QueueableForm as _;
use bir_app_core::{BirApp, BranchFacts, Draft, FieldValue, NewTaxpayer, OpenReturn, SendStage};
use serde_json::{Value, json};

const TEST_KEY: &str = "0000000000000000000000000000000000000000000000000000000000000000";
const REASON: &str = "abandoned_no_bir_filing";
const AUDIT: &str = "Abandoned claim released (abandoned_no_bir_filing); human confirmed nothing reached BIR";
const YEAR: u16 = 2026;
const RECENT: &str = "This return was handed to the send queue moments ago and may still be on its way to BIR. Try again after ";
const LONG_AGO: &str = "1999-01-01T00:00:00Z";

/// A fresh database with the demo taxpayer; the app, a second connection
/// to the same file (the worker's side) and the branch TIN.
fn open(dir: &std::path::Path) -> (BirApp, Database, String) {
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
    let worker = Database::open(&path).expect("a second connection");
    (app, worker, tp.branches[0].tin.clone())
}

/// Queue the September 1601C; its return id.
fn queue_1601c(app: &BirApp, tin: &str) -> i64 {
    let opened = app.open_return(&OpenReturn { tin: tin.into(), form_code: "1601C".into(), year: YEAR.into(), period: 9, key: None }).expect("open 1601C");
    let mut edits = BTreeMap::new();
    edits.insert("frm1601c:TaxWithheld_1".to_string(), FieldValue::Tick(true));
    edits.insert("frm1601c:CatAgent_P".to_string(), FieldValue::Tick(true));
    edits.insert("frm1601c:txtTax25".to_string(), FieldValue::from("150.00"));
    let layout = app.form_layout("1601C").expect("layout");
    for item in layout.parts.iter().flat_map(|p| p.items.iter()).filter(|i| i.no == "14") {
        for f in item.fields.iter().filter(|f| f.kind == "money" && !f.computed) {
            edits.insert(f.key.clone(), FieldValue::from("50000.00"));
        }
    }
    let filled = app.fill("1601C", &opened.draft, &edits, &[]).expect("fill");
    queue(app, "1601C", filled.draft)
}

/// Queue the Q1 2551Q (Item 13 answered).
fn queue_2551q(app: &BirApp, tin: &str) -> i64 {
    let opened = app.open_return(&OpenReturn { tin: tin.into(), form_code: "2551Q".into(), year: YEAR.into(), period: 1, key: None }).expect("open 2551Q");
    let mut draft = opened.draft;
    draft.0["item_13_election"] = "graduated".into();
    queue(app, "2551Q", draft)
}

/// Queue the January 0619E, a form on the generic queue.
fn queue_0619e(app: &BirApp, tin: &str) -> i64 {
    let opened = app.open_return(&OpenReturn { tin: tin.into(), form_code: "0619E".into(), year: YEAR.into(), period: 1, key: None }).expect("open 0619E");
    queue(app, "0619E", opened.draft)
}

fn queue(app: &BirApp, code: &str, draft: Draft) -> i64 {
    let checked = app.validate(code, &draft).expect("validate");
    assert!(checked.errors.is_empty(), "{code}: {:?}", checked.errors);
    let saved = app.save(code, &checked.draft).expect("save");
    let draft = saved.draft.unwrap_or(checked.draft);
    app.queue_return(code, &draft).expect("queue")
}

/// Claim the queued 1601C and mark its PUT started, as the worker does
/// right before an upload whose outcome then stays unknown.
fn claim_1601c(worker: &Database, tin: &str) {
    let queued = worker.get_1601c_draft(tin, YEAR, 9).unwrap().unwrap();
    let Claim1601CSubmissionResult::Claimed { token, .. } = worker
        .claim_queued_1601c_submission(tin, YEAR, 9, &queued.queued_submission_fingerprint, &queued.next_retry_at, queued.submission_attempts)
        .unwrap()
    else {
        panic!("the 1601C should claim");
    };
    assert!(worker.mark_claimed_1601c_put_started(tin, YEAR, 9, &token).unwrap());
}

fn claim_2551q(worker: &Database, tin: &str) {
    let queued = worker.get_2551q_draft(tin, YEAR, 1).unwrap().unwrap();
    let Claim2551QSubmissionResult::Claimed { token, .. } = worker
        .claim_queued_2551q_submission(tin, YEAR, 1, &queued.queued_submission_fingerprint, &queued.next_retry_at, queued.submission_attempts)
        .unwrap()
    else {
        panic!("the 2551Q should claim");
    };
    assert!(worker.mark_claimed_2551q_put_started(tin, YEAR, 1, &token).unwrap());
}

fn claim_0619e(worker: &Database, tin: &str) {
    let queued: Form0619EDraft = worker.get_queueable_draft(tin, YEAR, 1).unwrap().unwrap();
    let lifecycle = queued.lifecycle();
    let ClaimQueueableResult::Claimed { token, .. } = worker
        .claim_queued_queueable::<Form0619EDraft>(tin, YEAR, 1, &lifecycle.queued_submission_fingerprint, &lifecycle.next_retry_at, lifecycle.submission_attempts)
        .unwrap()
    else {
        panic!("the 0619E should claim");
    };
    assert!(worker.mark_claimed_queueable_put_started::<Form0619EDraft>(tin, YEAR, 1, &token).unwrap());
}

/// Release with the claim `queued_send_state` shows, as a screen does.
fn release(app: &BirApp, tin: &str, code: &str, slot: i64, reason: &str) -> Result<bool, String> {
    let seen = send_state(app, tin, code, slot)["claim"].as_str().map(str::to_owned);
    release_seen(app, tin, code, slot, seen.as_deref(), reason)
}

fn release_seen(app: &BirApp, tin: &str, code: &str, slot: i64, seen: Option<&str>, reason: &str) -> Result<bool, String> {
    app.release_abandoned_claim(tin, code, YEAR.into(), Some(slot), seen, reason).map_err(|error| error.to_string())
}

/// `queued_send_state`'s answer: `{stage, reason, claim, releasable}`.
fn send_state(app: &BirApp, tin: &str, code: &str, slot: i64) -> Value {
    app.call("queued_send_state", vec![tin.into(), code.into(), YEAR.into(), slot.into()]).expect("queued_send_state")
}

fn row(worker: &Database, id: i64) -> (String, String) {
    worker.draft_status_and_json(id).unwrap().expect("the return stays stored")
}

/// Set one field of return `id`'s stored JSON (and, with `status`, its
/// status column and JSON status), by an exact-JSON compare-and-swap, as
/// the cron tests move `next_retry_at`.
fn set_field(worker: &Database, id: i64, key: &str, value: Value) {
    let (status, json) = row(worker, id);
    let mut data: Value = serde_json::from_str(&json).unwrap();
    data[key] = value;
    let status = if key == "status" { data[key].as_str().unwrap().to_owned() } else { status };
    assert!(worker.replace_draft_json_if_unchanged(id, &json, &status, &data.to_string()).unwrap());
}

/// The claim's lease (`null`: none recorded).
fn set_lease(worker: &Database, id: i64, lease_until: Value) {
    set_field(worker, id, "submission_claim_lease_until", lease_until);
}

/// The claim's lease ran out long ago.
fn expire_lease(worker: &Database, id: i64) {
    set_lease(worker, id, json!(LONG_AGO));
}

/// No TIN in a message the person sees: neither dashed nor as a run of
/// nine or more digits.
fn assert_no_tin(message: &str) {
    let undashed: String = message.chars().filter(|c| *c != '-').collect();
    let mut run = 0;
    for c in undashed.chars() {
        run = if c.is_ascii_digit() { run + 1 } else { 0 };
        assert!(run < 9, "a TIN-like number in: {message}");
    }
}

/// The in-flight job key of queued return `id`, built from the worker's own
/// list of queued rows.
fn job_key(worker: &Database, id: i64) -> String {
    let queued = worker.list_all_queued_submissions().unwrap();
    queue_job_key(queued.iter().find(|summary| summary.id == id).expect("a queued row"))
}

/// The stage `send_states` shows for return `id`.
fn shown(app: &BirApp, id: i64) -> Option<SendStage> {
    app.send_states().unwrap().get(&id).cloned()
}

/// A refusal that changes nothing: `expected` starts the message.
fn assert_refused(app: &BirApp, worker: &Database, tin: &str, code: &str, slot: i64, id: i64, expected: &str) {
    let state = send_state(app, tin, code, slot);
    assert_eq!(state["releasable"], false, "{state}");
    let before = row(worker, id);
    let refused = release(app, tin, code, slot, REASON).expect_err("refused");
    assert!(refused.starts_with(expected), "{refused}");
    assert_no_tin(&refused);
    assert_eq!(row(worker, id), before);
}

/// Right after the claim: not releasable, and a release is refused with
/// the try-again time and changes nothing.
fn assert_refused_while_the_lease_is_live(app: &BirApp, worker: &Database, tin: &str, code: &str, slot: i64, id: i64) {
    assert_refused(app, worker, tin, code, slot, id, RECENT);
    let refused = release(app, tin, code, slot, REASON).unwrap_err();
    let after = refused.strip_prefix(RECENT).unwrap();
    assert!(after.len() == "HH:MM.".len() && after.ends_with('.'), "{refused}");
}

#[test]
fn abandoned_claims() {
    // SAFETY: set once, before any database opens.
    unsafe { std::env::set_var("BUWIZ_DB_KEY", TEST_KEY) };
    a_claimed_1601c_is_released_to_draft_with_the_audit_note();
    a_claimed_2551q_is_released_to_draft_with_the_audit_note();
    a_claimed_generic_return_is_released_to_draft_with_the_audit_note();
    a_wrong_reason_a_sent_return_and_an_unclaimed_queue_are_refused();
    a_claim_with_no_lease_recorded_waits_out_the_claim_lease();
    the_release_follows_who_runs_the_queue();
    a_return_being_sent_reads_sending();
    a_real_send_in_flight_blocks_the_release();
    the_release_is_for_the_claim_the_person_saw_and_readable_times();
    a_missing_return_or_an_out_of_range_period_releases_nothing();
}

fn a_claimed_1601c_is_released_to_draft_with_the_audit_note() {
    let dir = tempfile::tempdir().unwrap();
    let (app, worker, tin) = open(dir.path());
    let id = queue_1601c(&app, &tin);
    claim_1601c(&worker, &tin);
    assert_refused_while_the_lease_is_live(&app, &worker, &tin, "1601C", 9, id);
    expire_lease(&worker, id);

    let stuck = send_state(&app, &tin, "1601C", 9);
    assert_eq!(stuck["stage"], "NeedsAttention", "{stuck}");
    assert_eq!(stuck["releasable"], true, "{stuck}");
    assert!(stuck["claim"].is_string(), "{stuck}");
    assert!(stuck["reason"].as_str().is_some_and(|r| !r.is_empty()), "{stuck}");
    assert!(matches!(app.send_states().unwrap().get(&id), Some(SendStage::NeedsAttention { .. })));

    assert_eq!(release(&app, &tin, "1601C", 9, REASON), Ok(true));
    let released = worker.get_1601c_draft(&tin, YEAR, 9).unwrap().unwrap();
    assert_eq!(row(&worker, id).0, "Draft");
    assert_eq!(format!("{:?}", released.status), "Draft");
    assert!(released.submission_claim_token.is_none());
    assert!(released.submission_claimed_at.is_none());
    assert_eq!(released.submission_error.as_deref(), Some(AUDIT));
    assert_eq!(send_state(&app, &tin, "1601C", 9), json!({ "stage": null, "reason": null, "claim": null, "releasable": false }));

    // Nothing left to release: told apart from a release.
    assert_eq!(release(&app, &tin, "1601C", 9, REASON), Ok(false));
}

fn a_claimed_2551q_is_released_to_draft_with_the_audit_note() {
    let dir = tempfile::tempdir().unwrap();
    let (app, worker, tin) = open(dir.path());
    let id = queue_2551q(&app, &tin);
    claim_2551q(&worker, &tin);
    assert_refused_while_the_lease_is_live(&app, &worker, &tin, "2551Q", 1, id);
    expire_lease(&worker, id);

    // A 2551Q's reason is its `last_error`: stuck reads like the others.
    let stuck = send_state(&app, &tin, "2551Q", 1);
    assert_eq!(stuck["stage"], "NeedsAttention", "{stuck}");
    let note = worker.get_2551q_draft(&tin, YEAR, 1).unwrap().unwrap().last_error.expect("the claim's note");
    assert_eq!(stuck["reason"], note.as_str(), "{stuck}");
    assert_eq!(stuck["releasable"], true, "{stuck}");

    assert_eq!(release(&app, &tin, "2551Q", 1, REASON), Ok(true));
    let released = worker.get_2551q_draft(&tin, YEAR, 1).unwrap().unwrap();
    assert_eq!(row(&worker, id).0, "Draft");
    assert_eq!(format!("{:?}", released.status), "Draft");
    assert!(released.submission_claim_token.is_none());
    assert!(released.submission_claimed_at.is_none());
    assert_eq!(released.last_error.as_deref(), Some(AUDIT));
    assert_eq!(send_state(&app, &tin, "2551Q", 1), json!({ "stage": null, "reason": null, "claim": null, "releasable": false }));
}

fn a_claimed_generic_return_is_released_to_draft_with_the_audit_note() {
    let dir = tempfile::tempdir().unwrap();
    let (app, worker, tin) = open(dir.path());
    let id = queue_0619e(&app, &tin);
    claim_0619e(&worker, &tin);
    assert_refused_while_the_lease_is_live(&app, &worker, &tin, "0619E", 1, id);
    expire_lease(&worker, id);

    let stuck = send_state(&app, &tin, "0619E", 1);
    assert_eq!(stuck["stage"], "NeedsAttention", "{stuck}");
    assert_eq!(stuck["releasable"], true, "{stuck}");

    assert_eq!(release(&app, &tin, "0619E", 1, REASON), Ok(true));
    let released: Form0619EDraft = worker.get_queueable_draft(&tin, YEAR, 1).unwrap().unwrap();
    assert_eq!(row(&worker, id).0, "Draft");
    assert_eq!(format!("{:?}", released.lifecycle().status), "Draft");
    assert!(released.lifecycle().is_unclaimed());
    assert_eq!(released.lifecycle().submission_error.as_deref(), Some(AUDIT));
    assert_eq!(send_state(&app, &tin, "0619E", 1), json!({ "stage": null, "reason": null, "claim": null, "releasable": false }));
}

fn a_wrong_reason_a_sent_return_and_an_unclaimed_queue_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (app, worker, tin) = open(dir.path());

    // A claimed return with any other reason stays as it is.
    let id = queue_1601c(&app, &tin);
    claim_1601c(&worker, &tin);
    expire_lease(&worker, id);
    let before = row(&worker, id);
    for reason in ["", "abandoned", "ABANDONED_NO_BIR_FILING"] {
        let refused = release(&app, &tin, "1601C", 9, reason);
        assert!(refused.as_ref().is_err_and(|e| e.contains(REASON)), "{reason:?}: {refused:?}");
        assert_eq!(row(&worker, id), before);
    }
    assert_eq!(send_state(&app, &tin, "1601C", 9)["stage"], "NeedsAttention");
    assert_eq!(send_state(&app, &tin, "1601C", 9)["releasable"], true);

    // An unclaimed queue is cancelled, not released.
    let id = queue_0619e(&app, &tin);
    let before = row(&worker, id);
    assert_eq!(before.0, "Queued");
    assert!(release(&app, &tin, "0619E", 1, REASON).is_err());
    assert_eq!(row(&worker, id), before);
    assert_eq!(send_state(&app, &tin, "0619E", 1)["stage"], "Queued");
    assert_eq!(send_state(&app, &tin, "0619E", 1)["releasable"], false);
    assert_refused(&app, &worker, &tin, "0619E", 1, id, "This 0619E return is queued but the queue has not claimed it");

    let id = queue_2551q(&app, &tin);
    let before = row(&worker, id);
    assert!(release(&app, &tin, "2551Q", 1, REASON).is_err());
    assert_eq!(row(&worker, id), before);
    assert_eq!(send_state(&app, &tin, "2551Q", 1)["releasable"], false);

    // A sent, confirmed or paid return is never released.
    app.demo_set_send_stage(id, &SendStage::Sent { at: None, receipt: None }).unwrap();
    let before = row(&worker, id);
    assert_eq!(before.0, "Submitted");
    assert_eq!(send_state(&app, &tin, "2551Q", 1)["releasable"], false);
    let refused = release(&app, &tin, "2551Q", 1, REASON);
    assert!(refused.as_ref().is_err_and(|e| e.contains("Submitted")), "{refused:?}");
    assert_eq!(row(&worker, id), before);
    for status in ["Confirmed", "Paid"] {
        set_field(&worker, id, "status", json!(status));
        let before = row(&worker, id);
        assert_eq!(before.0, status);
        assert_eq!(send_state(&app, &tin, "2551Q", 1)["releasable"], false);
        let refused = release(&app, &tin, "2551Q", 1, REASON);
        assert!(refused.as_ref().is_err_and(|e| e.contains("Submitted, Confirmed, or Paid")), "{status}: {refused:?}");
        assert_eq!(row(&worker, id), before);
    }

    // A plain draft: nothing to release.
    let opened = app.open_return(&OpenReturn { tin: tin.clone(), form_code: "0619E".into(), year: YEAR.into(), period: 2, key: None }).expect("open 0619E");
    let checked = app.validate("0619E", &opened.draft).expect("validate");
    let saved = app.save("0619E", &checked.draft).expect("save");
    let id = saved.id.expect("a saved id");
    assert_eq!(row(&worker, id).0, "Draft");
    assert_eq!(send_state(&app, &tin, "0619E", 2), json!({ "stage": null, "reason": null, "claim": null, "releasable": false }));
    assert_eq!(release(&app, &tin, "0619E", 2, REASON), Ok(false));
}

fn a_claim_with_no_lease_recorded_waits_out_the_claim_lease() {
    let dir = tempfile::tempdir().unwrap();
    let (app, worker, tin) = open(dir.path());
    let id = queue_1601c(&app, &tin);
    claim_1601c(&worker, &tin);
    set_lease(&worker, id, Value::Null);

    // `claimed_at` plus the claim lease stands in for the missing lease.
    assert_refused_while_the_lease_is_live(&app, &worker, &tin, "1601C", 9, id);
    set_field(&worker, id, "submission_claimed_at", json!(LONG_AGO));
    assert_eq!(send_state(&app, &tin, "1601C", 9)["releasable"], true);
    assert_eq!(release(&app, &tin, "1601C", 9, REASON), Ok(true));
    assert_eq!(row(&worker, id).0, "Draft");

    // The demo hook's claimed rows have no lease either: the same wait.
    let id = queue_0619e(&app, &tin);
    app.demo_set_send_stage(id, &SendStage::NeedsAttention { reason: "Needs attention: dropped".into() }).unwrap();
    assert_refused_while_the_lease_is_live(&app, &worker, &tin, "0619E", 1, id);
}

fn the_release_follows_who_runs_the_queue() {
    let dir = tempfile::tempdir().unwrap();
    let (app, worker, tin) = open(dir.path());
    let id = queue_1601c(&app, &tin);
    claim_1601c(&worker, &tin);
    expire_lease(&worker, id);

    // This process runs the queue and its worker has the return's job.
    let lease = WorkerLease::try_acquire(app.database_path()).unwrap().expect("this process runs the queue");
    let key = job_key(&worker, id);
    assert!(get_active_jobs().lock().unwrap().insert(key.clone()));
    assert_refused(&app, &worker, &tin, "1601C", 9, id, "This return is being sent right now. Wait for the send to finish, then look at the return again. If it never finishes, quit and reopen the app first.");
    assert!(get_active_jobs().lock().unwrap().remove(&key));
    // Its worker is idle: releasable.
    assert_eq!(send_state(&app, &tin, "1601C", 9)["releasable"], true);
    drop(lease);

    // No process holds the lease, but a worker in this process (one can run
    // without it) has the job: refused as well.
    assert!(get_active_jobs().lock().unwrap().insert(key.clone()));
    assert_refused(&app, &worker, &tin, "1601C", 9, id, "This return is being sent right now.");
    assert!(get_active_jobs().lock().unwrap().remove(&key));

    // Another process runs the queue: it holds the worker lock file. Taken
    // here on its own handle, not through `WorkerLease`, so this process is
    // not recorded as the holder, exactly as for another process.
    let other = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(worker_lock_path(app.database_path()))
        .unwrap();
    other.try_lock().expect("the other window's lock");
    assert_refused(&app, &worker, &tin, "1601C", 9, id, "Another open copy of this app is running the send queue");
    // The guard comes first: any stored return is refused, a plain draft
    // too; a missing one still answers that there was nothing to release.
    let opened = app.open_return(&OpenReturn { tin: tin.clone(), form_code: "0619E".into(), year: YEAR.into(), period: 2, key: None }).expect("open 0619E");
    let saved = app.save("0619E", &app.validate("0619E", &opened.draft).expect("validate").draft).expect("save");
    let draft_id = saved.id.expect("a saved id");
    assert_refused(&app, &worker, &tin, "0619E", 2, draft_id, "Another open copy of this app is running the send queue");
    assert_eq!(release_seen(&app, &tin, "1601C", 3, None, REASON), Ok(false));
    drop(other);

    // Nobody runs the queue: reading the state changes nothing, and the
    // release goes through.
    let before = row(&worker, id);
    assert_eq!(send_state(&app, &tin, "1601C", 9)["releasable"], true);
    assert_eq!(row(&worker, id), before);
    assert_eq!(release(&app, &tin, "1601C", 9, REASON), Ok(true));
    assert!(WorkerLease::try_acquire(app.database_path()).unwrap().is_some(), "the probe let the lease go");
}

fn the_release_is_for_the_claim_the_person_saw_and_readable_times() {
    let dir = tempfile::tempdir().unwrap();
    let (app, worker, tin) = open(dir.path());
    let id = queue_0619e(&app, &tin);
    claim_0619e(&worker, &tin);
    expire_lease(&worker, id);

    // The claim changed since the person looked.
    let before = row(&worker, id);
    for seen in [None, Some("2026-01-01T00:00:00+00:00")] {
        let refused = release_seen(&app, &tin, "0619E", 1, seen, REASON).expect_err("another claim");
        assert_eq!(refused, "This return changed since you looked; open it again.");
        assert_eq!(row(&worker, id), before);
    }

    // An unreadable lease falls back to `claimed_at` plus the claim lease.
    set_lease(&worker, id, json!("not a time"));
    set_field(&worker, id, "submission_claimed_at", json!(chrono::Utc::now().to_rfc3339()));
    assert_refused_while_the_lease_is_live(&app, &worker, &tin, "0619E", 1, id);
    set_field(&worker, id, "submission_claimed_at", json!(LONG_AGO));
    assert_eq!(send_state(&app, &tin, "0619E", 1)["releasable"], true);

    // Neither time readable: never released here.
    set_field(&worker, id, "submission_claimed_at", json!("not a time either"));
    assert_refused(&app, &worker, &tin, "0619E", 1, id, "This return's claim times cannot be read, so it is not safe to release it here. Contact support.");
}

fn a_missing_return_or_an_out_of_range_period_releases_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (app, worker, tin) = open(dir.path());
    let id = queue_1601c(&app, &tin);
    claim_1601c(&worker, &tin);
    expire_lease(&worker, id);
    let before = row(&worker, id);

    // No March return: nothing to release, bir-core not asked.
    assert_eq!(release_seen(&app, &tin, "1601C", 3, None, "not the reason"), Ok(false));
    for (code, slot, expected) in [("1601C", 0, "1601C months are 1-12"), ("1601C", 13, "1601C months are 1-12"), ("1601C", 21, "1601C months are 1-12"), ("2551Q", 0, "2551Q quarters are 1-4"), ("2551Q", 5, "2551Q quarters are 1-4")] {
        let refused = release_seen(&app, &tin, code, slot, None, REASON).expect_err("out of range");
        assert!(refused.starts_with(expected), "{refused}");
        assert!(app.call("queued_send_state", vec![tin.clone().into(), code.into(), YEAR.into(), slot.into()]).is_err());
    }
    assert_eq!(row(&worker, id), before);
}

fn a_return_being_sent_reads_sending() {
    let dir = tempfile::tempdir().unwrap();
    let (app, worker, tin) = open(dir.path());
    let ids = [
        ("1601C", 9, queue_1601c(&app, &tin)),
        ("2551Q", 1, queue_2551q(&app, &tin)),
        ("0619E", 1, queue_0619e(&app, &tin)),
    ];
    claim_1601c(&worker, &tin);
    claim_2551q(&worker, &tin);
    claim_0619e(&worker, &tin);

    // Claimed and nothing in flight here: the claim's note says check BIR.
    for (code, slot, id) in ids {
        assert!(matches!(shown(&app, id), Some(SendStage::NeedsAttention { .. })), "{code}");
        assert_eq!(send_state(&app, &tin, code, slot)["stage"], "NeedsAttention", "{code}");
    }
    // This process's worker is sending them: they read Sending, and are
    // not releasable.
    let keys: Vec<String> = ids.iter().map(|(_, _, id)| job_key(&worker, *id)).collect();
    for key in &keys {
        assert!(get_active_jobs().lock().unwrap().insert(key.clone()));
    }
    for (code, slot, id) in ids {
        assert_eq!(shown(&app, id), Some(SendStage::Sending), "{code}");
        let state = send_state(&app, &tin, code, slot);
        assert_eq!(state["stage"], "Sending", "{code}: {state}");
        assert_eq!(state["reason"], Value::Null, "{code}: {state}");
        assert_eq!(state["releasable"], false, "{code}: {state}");
    }
    for key in &keys {
        assert!(get_active_jobs().lock().unwrap().remove(key));
    }
    for (code, slot, id) in ids {
        assert!(matches!(shown(&app, id), Some(SendStage::NeedsAttention { .. })), "{code}");
        assert_eq!(send_state(&app, &tin, code, slot)["stage"], "NeedsAttention", "{code}");
    }
}

/// A relay whose upload waits until the test lets it finish.
struct HeldTransport {
    finish: tokio::sync::Notify,
}

impl SubmissionTransport for HeldTransport {
    type Session = ();

    fn open_session<'a>(&'a self, _form_type: &'a str, _tin: &'a str) -> Pin<Box<dyn Future<Output = Result<(), TransportError>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }

    fn store_session<'a>(&'a self, _session: (), _filename: &'a str, _payload: &'a [u8]) -> Pin<Box<dyn Future<Output = Result<(), TransportError>> + Send + 'a>> {
        Box::pin(async move {
            self.finish.notified().await;
            Ok(())
        })
    }

    fn store_session_reported<'a>(&'a self, session: (), filename: &'a str, payload: &'a [u8]) -> Pin<Box<dyn Future<Output = Result<StoreReport, StoreFailure>> + Send + 'a>> {
        Box::pin(async move {
            self.store_session(session, filename, payload).await.map_err(StoreFailure::MaybeSent)?;
            Ok(StoreReport { receipt: None, dry_run: true })
        })
    }
}

/// Wait (up to 5 s) until `done` answers true.
fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    for _ in 0..500 {
        if done() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("timed out waiting until {what}");
}

fn a_real_send_in_flight_blocks_the_release() {
    for (code, slot) in [("1601C", 9), ("2551Q", 1)] {
        let dir = tempfile::tempdir().unwrap();
        let (app, side, tin) = open(dir.path());
        let id = if code == "1601C" { queue_1601c(&app, &tin) } else { queue_2551q(&app, &tin) };
        // This process runs the queue, with the real worker on its own
        // connection, as the app's worker thread does.
        let lease = WorkerLease::try_acquire(app.database_path()).unwrap().expect("this process runs the queue");
        let worker = Arc::new(Mutex::new(Database::open(app.database_path()).expect("the worker's connection")));
        let transport = Arc::new(HeldTransport { finish: tokio::sync::Notify::new() });
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(run_queue_tick(worker.clone(), &transport));

        // The upload has started and is still pending.
        wait_until("the upload started", || {
            let data: Value = serde_json::from_str(&row(&side, id).1).unwrap();
            data["submission_put_started_at"].is_string()
        });
        // Past the claim's minimum wait, so only the in-flight send holds it.
        expire_lease(&side, id);
        let state = send_state(&app, &tin, code, slot);
        assert_eq!(state["stage"], "Sending", "{code}: {state}");
        assert_eq!(state["releasable"], false, "{code}: {state}");
        assert_eq!(shown(&app, id), Some(SendStage::Sending), "{code}");
        let before = row(&side, id);
        let refused = release(&app, &tin, code, slot, REASON).expect_err("refused while the send runs");
        assert!(refused.starts_with("This return is being sent right now."), "{code}: {refused}");
        assert_eq!(row(&side, id), before);

        // The send finishes: the worker files it as Submitted.
        transport.finish.notify_one();
        wait_until("the send finished", || get_active_jobs().lock().unwrap().is_empty());
        assert_eq!(row(&side, id).0, "Submitted", "{code}");
        let state = send_state(&app, &tin, code, slot);
        assert_eq!(state["stage"], "Sent", "{code}: {state}");
        assert_eq!(state["releasable"], false, "{code}: {state}");
        let before = row(&side, id);
        let refused = release(&app, &tin, code, slot, REASON).expect_err("a sent return is never released");
        assert!(refused.contains("Submitted"), "{code}: {refused}");
        assert_eq!(row(&side, id), before);
        drop(lease);
    }
}
