//! Moving a deadline through `BirApp`: the calendar feed shows the new date
//! marked as moved, with the note; putting it back restores BIR's date.

use bir_app_core::{BirApp, BranchFacts, NewTaxpayer};

const TEST_KEY: &str = "0000000000000000000000000000000000000000000000000000000000000000";

#[test]
fn a_moved_deadline_shows_in_the_calendar_and_goes_back() {
    // SAFETY: set before any thread of this test reads it (one test per process).
    unsafe { std::env::set_var("BIR_DB_KEY", TEST_KEY) };
    let dir = tempfile::tempdir().unwrap();
    let app = BirApp::open_at(dir.path().join("bir_data.db")).unwrap();
    let taxpayer = app
        .create_taxpayer(&NewTaxpayer {
            tin_root: "000123453".into(),
            name: "Juana Santos".into(),
            taxpayer_type: "Individual".into(),
            birth_date: Some("1990-01-15".into()),
            head_office: BranchFacts {
                rdo_code: "043".into(),
                registered_address: "1 Sample Street, Pasig City".into(),
                zip_code: "1600".into(),
                phone: String::new(),
                email: String::new(),
            },
        })
        .unwrap();
    let tin = taxpayer.branches[0].tin.clone();
    app.call("set_yearly_forms", vec![tin.as_str().into(), 2026.into(), serde_json::json!(["2551Q"])]).unwrap();
    let q3 = |app: &BirApp| {
        app.deadlines_between("2026-10-01", "2026-12-31")
            .unwrap()
            .into_iter()
            .find(|d| d.tin == tin && d.form_code == "2551Q" && d.taxable_year == 2026 && d.period == 3)
            .expect("2551Q Q3 2026 is in the feed")
    };

    let before = q3(&app);
    assert_eq!(before.period_key.as_deref(), Some("2026-Q3"));
    assert_eq!(before.original_due_date.as_deref(), Some("2026-10-25"));
    assert!(!before.is_moved());
    let bir_date = before.due_date.clone();

    let moved = app.move_deadline("2551Q", "2026-Q3", "2026-11-03", Some("RMC 99-2026")).unwrap();
    assert_eq!(moved.due.as_deref(), Some("2026-11-03"));
    assert_eq!(moved.was.as_deref(), Some(bir_date.as_str()));
    assert_eq!(moved.entry.kind, "moved");
    assert!(!moved.replaced);

    let row = q3(&app);
    assert!(row.is_moved());
    assert_eq!(row.due_date, "2026-11-03");
    assert_eq!(row.moved_reason.as_deref(), Some("RMC 99-2026"));

    // Moved again without a note: the same entry, the default reason.
    let again = app.move_deadline("2551Q", "2026-Q3", "2026-11-04", None).unwrap();
    assert!(again.replaced);
    assert_eq!(again.entry.id, moved.entry.id);
    let listed = app.deadline_overrides(Some(2026)).unwrap();
    let mine: Vec<_> = listed.overrides.iter().filter(|o| o.kind == "moved").collect();
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0].new_deadline, "2026-11-04");
    assert_eq!(mine[0].periods[0].period, "2026-Q3");
    assert!(!listed.unreadable);

    let restored = app.restore_deadline("2551Q", "2026-Q3").unwrap();
    assert_eq!(restored.due.as_deref(), Some(bir_date.as_str()));
    assert_eq!(restored.removed.len(), 1);
    let row = q3(&app);
    assert!(!row.is_moved());
    assert_eq!(row.due_date, bir_date);
    assert!(app.restore_deadline("2551Q", "2026-Q3").is_err(), "nothing left to put back");
    assert!(app.move_deadline("0605", "2026-09", "2026-11-03", None).is_err(), "an as-needed form has no calendar deadline");
}
