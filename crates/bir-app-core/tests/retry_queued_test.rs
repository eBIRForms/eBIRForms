//! A person's Retry on a stopped queued return through the host function
//! `retry_queued`: the same rule as `BirApp::retry` without try-anyway, for
//! a 1601C, a 2551Q and a generic form. A stuck return is lifted and reads a
//! waiting stage again, a press too soon answers when it may be pressed
//! again and changes nothing, and a Draft, a Sent or a claimed return is
//! refused.

use std::collections::BTreeMap;
use bir_app_core::bir_core::db::{Claim1601CSubmissionResult, ClaimQueueableResult, Database};
use bir_app_core::bir_core::forms::form_0619e::Form0619EDraft;
use bir_app_core::bir_core::forms::queueable::QueueableForm as _;
use bir_app_core::bir_core::smart_queue::{PATH_SETTING, PathBackoff};
use bir_app_core::{BirApp, BranchFacts, Draft, FieldValue, NewTaxpayer, OpenReturn, SendStage};
use chrono::{Duration, Utc};
use serde_json::{Value, json};

const TEST_KEY: &str = "0000000000000000000000000000000000000000000000000000000000000000";
const YEAR: u16 = 2026;
const STUCK: &str = "the login was refused";

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


/// `retry_queued`'s answer for the return in `slot`.
fn press(app: &BirApp, tin: &str, code: &str, slot: i64) -> Result<Value, String> {
    app.call("retry_queued", vec![tin.into(), code.into(), YEAR.into(), slot.into()]).map_err(|error| error.to_string())
}

/// `queued_send_state`'s stage name.
fn stage(app: &BirApp, tin: &str, code: &str, slot: i64) -> String {
    let state = app.call("queued_send_state", vec![tin.into(), code.into(), YEAR.into(), slot.into()]).expect("queued_send_state");
    state["stage"].as_str().unwrap_or("").to_owned()
}

fn row(worker: &Database, id: i64) -> (String, Value) {
    let (status, json) = worker.draft_status_and_json(id).unwrap().expect("stored");
    (status, serde_json::from_str(&json).unwrap())
}

/// Leave the return stopped, as three identical refusals leave it.
fn make_stuck(worker: &Database, id: i64) {
    let (status, json) = worker.draft_status_and_json(id).unwrap().unwrap();
    let mut data: Value = serde_json::from_str(&json).unwrap();
    data["smart_queue"] = json!({ "failures": 3, "kind": STUCK, "same_kind": 3, "stuck": STUCK });
    data["last_error"] = format!("Not sent (nothing reached BIR): {STUCK}").into();
    assert!(worker.replace_draft_json_if_unchanged(id, &json, &status, &data.to_string()).unwrap());
}

/// The stopped return is lifted, a second press waits, and nothing about
/// the person's try-anyway is ever stored.
fn lift_and_limit(app: &BirApp, worker: &Database, tin: &str, code: &str, slot: i64, id: i64) {
    make_stuck(worker, id);
    assert_eq!(stage(app, tin, code, slot), "NeedsYou");
    let before = row(worker, id).1;
    assert!(before["smart_queue"]["stuck"].is_string());

    let first = press(app, tin, code, slot).expect("pressed");
    assert_eq!(first["pressed"], true, "{first}");
    assert_eq!(first["going"], true, "{first}");
    assert_eq!(first["goes_at"], Value::Null);
    assert_eq!(first["press_again_at"], Value::Null);
    assert_eq!(first["words"], "Sending again now.");
    let lifted = row(worker, id);
    assert_eq!(lifted.0, "Queued");
    assert!(lifted.1["smart_queue"].get("stuck").is_none(), "Retry lifts the stop");
    assert_eq!(lifted.1["smart_queue"]["failures"], 3, "the failures stay");
    assert!(lifted.1["smart_queue"].get("try_anyway").is_none_or(Value::is_null));
    let waiting = stage(app, tin, code, slot);
    assert!(["Queued", "Behind", "Failed"].contains(&waiting.as_str()), "{waiting}");

    let second = press(app, tin, code, slot).expect("answered");
    assert_eq!(second["pressed"], false, "{second}");
    assert_eq!(second["going"], false, "{second}");
    let again = chrono::DateTime::parse_from_rfc3339(second["press_again_at"].as_str().expect("a time")).unwrap();
    let wait = again.timestamp() - Utc::now().timestamp();
    assert!((29 * 60..=30 * 60).contains(&wait), "{wait}");
    let words = second["words"].as_str().unwrap();
    assert!(words.starts_with("Retry was pressed a while ago. You can press it again in "), "{words}");
    assert_eq!(row(worker, id), lifted, "a press too soon changes nothing");
}

#[test]
fn retry_queued_lifts_a_stopped_return() {
    // SAFETY: set once, before any database opens.
    unsafe { std::env::set_var("BUWIZ_DB_KEY", TEST_KEY) };
    a_stopped_1601c_is_lifted();
    a_stopped_2551q_is_lifted();
    a_stopped_generic_return_is_lifted();
    the_queue_wide_wait_is_told_and_try_anyway_is_never_set();
    a_draft_a_sent_a_claimed_and_a_missing_return_are_refused();
    a_press_is_limited_to_three_a_day();
}

fn a_stopped_1601c_is_lifted() {
    let dir = tempfile::tempdir().unwrap();
    let (app, worker, tin) = open(dir.path());
    let id = queue_1601c(&app, &tin);
    lift_and_limit(&app, &worker, &tin, "1601C", 9, id);
}

fn a_stopped_2551q_is_lifted() {
    let dir = tempfile::tempdir().unwrap();
    let (app, worker, tin) = open(dir.path());
    let id = queue_2551q(&app, &tin);
    lift_and_limit(&app, &worker, &tin, "2551Q", 1, id);
}

fn a_stopped_generic_return_is_lifted() {
    let dir = tempfile::tempdir().unwrap();
    let (app, worker, tin) = open(dir.path());
    let id = queue_0619e(&app, &tin);
    lift_and_limit(&app, &worker, &tin, "0619E", 1, id);
}

fn the_queue_wide_wait_is_told_and_try_anyway_is_never_set() {
    let dir = tempfile::tempdir().unwrap();
    let (app, worker, tin) = open(dir.path());
    let id = queue_0619e(&app, &tin);
    make_stuck(&worker, id);
    let floor = Utc::now() + Duration::hours(1);
    let path = PathBackoff { failures: 2, next_try_at: Some(floor.to_rfc3339()), ended_at: Some(Utc::now().to_rfc3339()), ..PathBackoff::default() };
    worker.set_setting(PATH_SETTING, &path.to_setting()).unwrap();
    let answer = press(&app, &tin, "0619E", 1).expect("pressed");
    assert_eq!(answer["pressed"], true, "{answer}");
    let goes = chrono::DateTime::parse_from_rfc3339(answer["goes_at"].as_str().expect("a time")).unwrap();
    assert_eq!(goes.timestamp(), floor.timestamp());
    assert!(answer["words"].as_str().unwrap().starts_with("Noted. The BIR could not be reached a moment ago"), "{answer}");
    assert!(row(&worker, id).1["smart_queue"].get("try_anyway").is_none_or(Value::is_null), "not set");
    // The function takes the return's place and nothing else.
    assert!(app.call("retry_queued", vec![tin.clone().into(), "0619E".into(), YEAR.into(), 1_i64.into(), true.into()]).is_ok());
    assert!(row(&worker, id).1["smart_queue"].get("try_anyway").is_none_or(Value::is_null), "an extra argument is not read");
}

fn a_draft_a_sent_a_claimed_and_a_missing_return_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (app, worker, tin) = open(dir.path());
    let only_queued = "Only a queued return can be sent again.";
    let claimed = "BIR may already have this return. Check for BIR's confirmation email before sending it again.";

    // A Draft: queued, then cancelled back.
    let id = queue_0619e(&app, &tin);
    app.call("cancel_queued", vec![tin.clone().into(), "0619E".into(), YEAR.into(), 1_i64.into()]).expect("cancel");
    let before = row(&worker, id);
    assert_eq!(before.0, "Draft");
    assert_eq!(press(&app, &tin, "0619E", 1), Err(only_queued.to_owned()));
    assert_eq!(row(&worker, id), before);

    // A Sent return.
    let id = queue_2551q(&app, &tin);
    app.demo_set_send_stage(id, &SendStage::Sent { at: None, receipt: None }).expect("sent");
    let before = row(&worker, id);
    assert_eq!(before.0, "Submitted");
    assert_eq!(press(&app, &tin, "2551Q", 1), Err(only_queued.to_owned()));
    assert_eq!(row(&worker, id), before);

    // A claimed return, in each of the three kinds.
    let id = queue_1601c(&app, &tin);
    claim_1601c(&worker, &tin);
    make_stuck(&worker, id);
    let before = row(&worker, id);
    assert_eq!(press(&app, &tin, "1601C", 9), Err(claimed.to_owned()));
    assert_eq!(row(&worker, id), before);

    let id = queue_0619e(&app, &tin);
    claim_0619e(&worker, &tin);
    let before = row(&worker, id);
    assert_eq!(press(&app, &tin, "0619E", 1), Err(claimed.to_owned()));
    assert_eq!(row(&worker, id), before);

    // Nothing stored, or a period out of range.
    assert!(press(&app, &tin, "1601C", 2).unwrap_err().starts_with("No 1601C return for that period"));
    assert!(press(&app, &tin, "1601C", 13).is_err());
    assert!(press(&app, &tin, "2551Q", 5).is_err());
    assert!(press(&app, &tin, "9999Z", 1).is_err());
}

fn a_press_is_limited_to_three_a_day() {
    let dir = tempfile::tempdir().unwrap();
    let (app, worker, tin) = open(dir.path());
    let id = queue_2551q(&app, &tin);
    make_stuck(&worker, id);
    assert_eq!(press(&app, &tin, "2551Q", 1).unwrap()["pressed"], true);
    // Age the recorded presses, so the 30-minute gap has passed each time.
    for _ in 0..2 {
        let (status, json) = worker.draft_status_and_json(id).unwrap().unwrap();
        let mut data: Value = serde_json::from_str(&json).unwrap();
        let presses = data["smart_queue"]["retries"].as_array().cloned().unwrap_or_default();
        let aged: Vec<Value> = presses.iter().map(|p| json!((chrono::DateTime::parse_from_rfc3339(p.as_str().unwrap()).unwrap() - Duration::hours(1)).to_rfc3339())).collect();
        data["smart_queue"]["retries"] = aged.into();
        assert!(worker.replace_draft_json_if_unchanged(id, &json, &status, &data.to_string()).unwrap());
        assert_eq!(press(&app, &tin, "2551Q", 1).unwrap()["pressed"], true);
    }
    let (status, json) = worker.draft_status_and_json(id).unwrap().unwrap();
    let mut data: Value = serde_json::from_str(&json).unwrap();
    let aged: Vec<Value> = data["smart_queue"]["retries"].as_array().unwrap().iter().map(|p| json!((chrono::DateTime::parse_from_rfc3339(p.as_str().unwrap()).unwrap() - Duration::hours(1)).to_rfc3339())).collect();
    data["smart_queue"]["retries"] = aged.into();
    assert!(worker.replace_draft_json_if_unchanged(id, &json, &status, &data.to_string()).unwrap());
    let fourth = press(&app, &tin, "2551Q", 1).unwrap();
    assert_eq!(fourth["pressed"], false, "{fourth}");
    assert!(fourth["press_again_at"].is_string());
}
