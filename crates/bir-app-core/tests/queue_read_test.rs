//! A mini-app reads the submission queue while another app runs it: the
//! queue list and send stages need no worker lease (that is for sending).

use std::collections::BTreeMap;

use bir_app_core::bir_core::db::WorkerLease;
use bir_app_core::{BirApp, BranchFacts, FieldValue, NewTaxpayer, OpenReturn, SendStage};

const TEST_KEY: &str = "0000000000000000000000000000000000000000000000000000000000000000";

#[test]
fn the_queue_reads_while_another_app_holds_the_lease() {
    // SAFETY: tests set the key before any database opens.
    unsafe { std::env::set_var("BUWIZ_DB_KEY", TEST_KEY) };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bir_data.db");
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

    let worker = WorkerLease::try_acquire(&path).unwrap().expect("the other app's worker");
    assert!(app.try_queue_lease().unwrap().is_none(), "this process may not send");

    let queue = app.list_queue().expect("the queue reads without the lease");
    assert_eq!(queue.iter().map(|q| (q.id, q.form_code.as_str())).collect::<Vec<_>>(), [(id, "1601C")]);
    assert_eq!(queue[0].status, "Queued");
    let states = app.send_states().expect("send stages read without the lease");
    assert!(matches!(states.get(&id), Some(SendStage::Queued { .. })));
    drop(worker);
}
