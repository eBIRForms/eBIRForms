//! A profile box the taxpayer profile leaves blank (no email when the
//! taxpayer was created) opens empty and unlocked; the filer's edit to it
//! must be kept, not refused, and reported for writing back to the profile.

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard};

use bir_app_core::{BirApp, BranchFacts, FieldValue, NewTaxpayer, OpenReturn};

/// `BirApp` keeps one database per process, so these tests take turns.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

const TEST_KEY: &str = "0000000000000000000000000000000000000000000000000000000000000000";

fn app_with_taxpayer(email: &str) -> (tempfile::TempDir, BirApp, String) {
    // SAFETY: set before any thread of this test reads it.
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
                email: email.into(),
            },
        })
        .unwrap();
    let tin = taxpayer.branches[0].tin.clone();
    (dir, app, tin)
}

fn open(app: &BirApp, tin: &str, form: &str, year: i64, period: i64) -> bir_app_core::OpenedReturn {
    app.open_return(&OpenReturn { tin: tin.into(), form_code: form.into(), year, period, key: None }).unwrap()
}

/// The form's email box, as its layout names it.
fn email_key(app: &BirApp, form: &str) -> String {
    let layout = app.form_layout(form).unwrap();
    layout.parts.iter().flat_map(|p| p.items.iter()).flat_map(|i| i.fields.iter()).find(|f| f.kind == "email").unwrap().key.clone()
}

#[test]
fn a_blank_profile_email_is_filled_on_the_1701q() {
    let _serial = serial();
    let (_dir, app, tin) = app_with_taxpayer("");
    let opened = open(&app, &tin, "1701Q", 2026, 3);
    let key = &email_key(&app, "1701Q");
    let key = key.as_str();
    assert_eq!(opened.values.get(key).map(String::as_str).unwrap_or(""), "");
    assert!(!opened.locks.contains_key(key), "a blank profile box opens unlocked");

    let edits = BTreeMap::from([(key.to_owned(), FieldValue::Text("juana@example.com".into()))]);
    let filled = app.fill("1701Q", &opened.draft, &edits, &[]).unwrap();
    assert!(!filled.rejected.contains_key(key), "refused: {:?}", filled.rejected.get(key));
    assert_eq!(filled.values.get(key).map(String::as_str), Some("juana@example.com"));
    assert_eq!(filled.profile_blank, vec![key.to_owned()], "rejected {:?} extra {:?}", filled.rejected, filled.extra.keys().collect::<Vec<_>>());

    let checked = app.validate("1701Q", &filled.draft).unwrap();
    let email_errors: Vec<_> = checked.errors.iter().filter(|e| e.keys.iter().any(|k| k == key) || e.message.to_lowercase().contains("email")).collect();
    assert!(email_errors.is_empty(), "email errors: {email_errors:?}");
}

#[test]
fn a_profile_email_with_a_value_stays_refused() {
    let _serial = serial();
    let (_dir, app, tin) = app_with_taxpayer("juana@example.com");
    let opened = open(&app, &tin, "1701Q", 2026, 3);
    let key = &email_key(&app, "1701Q");
    let key = key.as_str();
    let edits = BTreeMap::from([(key.to_owned(), FieldValue::Text("other@example.com".into()))]);
    let filled = app.fill("1701Q", &opened.draft, &edits, &[]).unwrap();
    assert!(filled.rejected.contains_key(key));
    assert!(filled.profile_blank.is_empty());
}

/// Every form's blank profile email box takes the filer's value.
#[test]
fn every_forms_blank_profile_email_is_kept() {
    let _serial = serial();
    let (_dir, app, tin) = app_with_taxpayer("");
    let mut checked = 0;
    for entry in BirApp::form_catalog().unwrap() {
        let Ok(layout) = app.form_layout(&entry.code) else { continue };
        let Some(field) = layout
            .parts
            .iter()
            .flat_map(|p| p.items.iter())
            .flat_map(|i| i.fields.iter())
            .find(|f| f.kind == "email" && !f.computed)
        else {
            continue;
        };
        let Ok(opened) = app.open_return(&OpenReturn { tin: tin.clone(), form_code: entry.code.clone(), year: 2026, period: 1, key: None }) else { continue };
        if opened.locks.contains_key(&field.key) || !opened.profile.is_empty() && !opened.values.get(&field.key).is_some_and(|v| v.is_empty()) {
            continue;
        }
        let edits = BTreeMap::from([(field.key.clone(), FieldValue::Text("juana@example.com".into()))]);
        let filled = app.fill(&entry.code, &opened.draft, &edits, &[]).unwrap();
        assert!(!filled.rejected.contains_key(&field.key), "{}: refused {:?}", entry.code, filled.rejected.get(&field.key));
        let shown = filled.values.get(&field.key).cloned().unwrap_or_default();
        // 1604F and 1801 print the email in capitals (official).
        assert_eq!(shown.to_lowercase(), "juana@example.com", "{}", entry.code);
        checked += 1;
    }
    assert!(checked > 3, "only {checked} forms checked");
}
