//! A return Buwiz refused for its TIN is a Draft with its reason: it reads
//! `Refused`, `refused_returns` lists it, and `queued_send_state` gives a
//! screen its line and step instead of the raw reason.

use std::collections::BTreeMap;

use bir_app_core::{BirApp, BranchFacts, FieldValue, NewTaxpayer, OpenReturn, SendStage, TinRefusal};
use serde_json::Value;

const TEST_KEY: &str = "0000000000000000000000000000000000000000000000000000000000000000";
const NOT_ACTIVE: &str = "Not sent (nothing reached BIR): this TIN is not active on your account (tin_not_active)";

/// A database with the demo taxpayer and queued 1601C returns for
/// `months`; the app, the branch TIN and the return ids.
fn queued_1601c(dir: &std::path::Path, months: &[i64]) -> (BirApp, String, Vec<i64>) {
    // SAFETY: tests set the key before any database opens.
    unsafe { std::env::set_var("BUWIZ_DB_KEY", TEST_KEY) };
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
    let layout = app.form_layout("1601C").expect("layout");
    let mut ids = Vec::new();
    for &month in months {
        let opened = app.open_return(&OpenReturn { tin: tin.clone(), form_code: "1601C".into(), year: 2026, period: month, key: None }).expect("open 1601C");
        let mut edits = BTreeMap::new();
        edits.insert("frm1601c:TaxWithheld_1".to_string(), FieldValue::Tick(true));
        edits.insert("frm1601c:CatAgent_P".to_string(), FieldValue::Tick(true));
        edits.insert("frm1601c:txtTax25".to_string(), FieldValue::from("150.00"));
        for item in layout.parts.iter().flat_map(|p| p.items.iter()).filter(|i| i.no == "14") {
            for f in item.fields.iter().filter(|f| f.kind == "money" && !f.computed) {
                edits.insert(f.key.clone(), FieldValue::from("50000.00"));
            }
        }
        let filled = app.fill("1601C", &opened.draft, &edits, &[]).expect("fill");
        let checked = app.validate("1601C", &filled.draft).expect("validate");
        let saved = app.save("1601C", &checked.draft).expect("save");
        let draft = saved.draft.unwrap_or(checked.draft);
        ids.push(app.queue_return("1601C", &draft).expect("queue"));
    }
    (app, tin, ids)
}

fn send_state(app: &BirApp, tin: &str, month: i64) -> Value {
    app.call("queued_send_state", vec![tin.into(), "1601C".into(), 2026.into(), month.into()]).expect("queued_send_state")
}

#[test]
fn refused_drafts() {
    refused_returns_lists_only_refused_drafts();
    queued_send_state_reports_line_and_step_for_a_refused_draft();
}

fn refused_returns_lists_only_refused_drafts() {
    let dir = tempfile::tempdir().unwrap();
    let (app, _tin, ids) = queued_1601c(dir.path(), &[8, 9, 10]);
    let refused = SendStage::Refused { code: TinRefusal::NotActive, reason: NOT_ACTIVE.into() };
    app.demo_set_send_stage(ids[0], &refused).unwrap();
    let failed = SendStage::Failed { reason: "Not sent (nothing reached BIR): the relay closed early".into(), attempts: 1, next_retry_at: None };
    app.demo_set_send_stage(ids[1], &failed).unwrap();

    let states = app.send_states().unwrap();
    assert_eq!(states.get(&ids[0]), Some(&refused));
    assert!(matches!(states.get(&ids[1]), Some(SendStage::Failed { .. })));
    assert!(matches!(states.get(&ids[2]), Some(SendStage::Queued { .. })));

    let listed = app.refused_returns().unwrap();
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!(listed[0].row.id, ids[0]);
    assert_eq!(listed[0].row.status, "Draft");
    assert_eq!(listed[0].stage, refused);
    // A refused draft is a draft: it is not retried by hand either.
    assert!(app.retry_now(ids[0]).is_err());
}

fn queued_send_state_reports_line_and_step_for_a_refused_draft() {
    let dir = tempfile::tempdir().unwrap();
    let (app, tin, ids) = queued_1601c(dir.path(), &[8, 9, 10]);
    let plain = send_state(&app, &tin, 10);
    assert_eq!(plain["stage"], "Queued");
    for key in ["chip", "line", "next", "step"] {
        assert_eq!(plain[key], Value::Null, "{key}");
    }

    let needs_fix = "Not sent (nothing reached BIR): Sent back for a fix: A photo is too blurry to read. (tin_needs_fix)";
    app.demo_set_send_stage(ids[1], &SendStage::Refused { code: TinRefusal::NeedsFix, reason: needs_fix.into() }).unwrap();
    let refused = send_state(&app, &tin, 9);
    assert_eq!(refused["stage"], "Refused");
    assert_eq!(refused["reason"], needs_fix);
    assert_eq!(refused["chip"], "Sent back");
    assert_eq!(refused["line"], "Buwiz sent this TIN back for a fix: A photo is too blurry to read.");
    assert_eq!(refused["next"], "Fix what Buwiz asked for on your Buwiz account, then queue the return again. Your answers are kept.");
    assert_eq!(refused["step"], "fix");
    assert_eq!(refused["claim"], Value::Null);
    assert_eq!(refused["releasable"], false);

    // A queued row an older build left failed with a code reads its line too.
    app.demo_set_send_stage(ids[0], &SendStage::Failed { reason: NOT_ACTIVE.into(), attempts: 2, next_retry_at: None }).unwrap();
    let legacy = send_state(&app, &tin, 8);
    assert_eq!(legacy["stage"], "Failed");
    assert_eq!(legacy["chip"], "Not on your Buwiz account");
    assert_eq!(legacy["line"], "This TIN is not on your Buwiz account.");
    assert_eq!(legacy["step"], "add");
    println!("queued_send_state for a refused draft: {refused}");
}
