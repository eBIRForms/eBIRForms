//! The send stages the app shows come from what the queue worker writes on
//! the stored return; Retry only touches an unclaimed queued return.

use std::collections::BTreeMap;

use bir_app_core::{BirApp, BranchFacts, FieldValue, NewTaxpayer, OpenReturn, SendStage};

const TEST_KEY: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// A database with the demo taxpayer and one queued 1601C; its return id.
fn queued_1601c(dir: &std::path::Path) -> (BirApp, i64) {
    // SAFETY: tests set the key before any database opens.
    unsafe { std::env::set_var("BIR_DB_KEY", TEST_KEY) };
    let app = BirApp::open_at(dir.join("bir_data.db")).expect("open");
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
    let opened = app.open_return(&OpenReturn { tin, form_code: "1601C".into(), year: 2026, period: 9, key: None }).expect("open 1601C");
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
    let checked = app.validate("1601C", &filled.draft).expect("validate");
    let saved = app.save("1601C", &checked.draft).expect("save");
    let draft = saved.draft.unwrap_or(checked.draft);
    let id = app.queue_return("1601C", &draft).expect("queue");
    (app, id)
}

#[test]
fn send_stages() {
    stages_follow_the_stored_lifecycle_and_retry_is_guarded();
    a_demo_sent_return_carries_a_readable_receipt();
}

fn stages_follow_the_stored_lifecycle_and_retry_is_guarded() {
    let dir = tempfile::tempdir().unwrap();
    let (app, id) = queued_1601c(dir.path());
    assert!(matches!(app.send_states().unwrap().get(&id), Some(SendStage::Queued { .. })));

    let reason = "Not sent (nothing reached BIR): TIN not active (tin_not_active)".to_string();
    app.demo_set_send_stage(id, &SendStage::Failed { reason: reason.clone(), attempts: 1, next_retry_at: Some("2099-01-01T00:00:00Z".into()) }).unwrap();
    match app.send_states().unwrap().get(&id) {
        Some(SendStage::Failed { reason: r, next_retry_at, .. }) => {
            assert_eq!(r, &reason);
            assert_eq!(next_retry_at.as_deref(), Some("2099-01-01T00:00:00Z"));
        }
        other => panic!("expected Failed, got {other:?}"),
    }
    app.retry_now(id).unwrap();
    match app.send_states().unwrap().get(&id) {
        Some(SendStage::Failed { next_retry_at: Some(at), .. }) => assert!(at.as_str() < "2099"),
        other => panic!("expected Failed due now, got {other:?}"),
    }

    app.demo_set_send_stage(id, &SendStage::NeedsAttention { reason: "Needs attention: dropped".into() }).unwrap();
    assert!(matches!(app.send_states().unwrap().get(&id), Some(SendStage::NeedsAttention { .. })));
    assert!(app.retry_now(id).is_err(), "a return BIR may have is never resent by Retry");
}

/// One database per process (bir-app-core installs a global handle), so
/// the Sent case runs after the other on a fresh queued return.
fn a_demo_sent_return_carries_a_readable_receipt() {
    let dir = tempfile::tempdir().unwrap();
    let (app, id) = queued_1601c(dir.path());
    app.demo_set_send_stage(id, &SendStage::Sent { at: None, receipt: None }).unwrap();
    match app.send_states().unwrap().get(&id) {
        Some(SendStage::Sent { at: Some(_), receipt: Some(receipt) }) => {
            assert_eq!(receipt.jti.as_deref(), Some("demo-7f3a91c2"));
            assert!(receipt.is_test());
        }
        other => panic!("expected Sent with a receipt, got {other:?}"),
    }
    assert!(app.retry_now(id).is_err());
}
