//! A box the tax profile fills can be changed on one return without
//! touching the profile: the edit is kept (no "change it in the profile"
//! refusal, the second time too), the profile is never written back, and
//! "Use profile value" puts the profile's value back. "Save to profile too"
//! is the filer's explicit choice. The profile's birth date and business
//! facts are written through the typed API.

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard};

use bir_app_core::{BirApp, BranchFacts, FieldValue, NewTaxpayer, OpenReturn, OpenedReturn, ProfileDetails, TaxpayerUpdate};

/// `BirApp` keeps one database per process, so these tests take turns.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

const TEST_KEY: &str = "0000000000000000000000000000000000000000000000000000000000000000";
const ROOT: &str = "000123453";
const FORM: &str = "1701Q";

fn app_with_taxpayer(email: &str) -> (tempfile::TempDir, BirApp, String) {
    // SAFETY: set before any thread of this test reads it.
    unsafe { std::env::set_var("BUWIZ_DB_KEY", TEST_KEY) };
    let dir = tempfile::tempdir().unwrap();
    let app = BirApp::open_at(dir.path().join("bir_data.db")).unwrap();
    let taxpayer = app
        .create_taxpayer(&NewTaxpayer {
            tin_root: ROOT.into(),
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

fn open(app: &BirApp, tin: &str, period: i64) -> OpenedReturn {
    app.open_return(&OpenReturn { tin: tin.into(), form_code: FORM.into(), year: 2026, period, key: None }).unwrap()
}

/// The form's email box, as its layout names it.
fn email_key(app: &BirApp) -> String {
    let layout = app.form_layout(FORM).unwrap();
    layout.parts.iter().flat_map(|p| p.items.iter()).flat_map(|i| i.fields.iter()).find(|f| f.kind == "email").unwrap().key.clone()
}

fn text(key: &str, value: &str) -> BTreeMap<String, FieldValue> {
    BTreeMap::from([(key.to_owned(), FieldValue::Text(value.into()))])
}

fn shown(values: &BTreeMap<String, String>, key: &str) -> String {
    values.get(key).cloned().unwrap_or_default().to_lowercase()
}

fn profile_email(app: &BirApp) -> String {
    app.get_taxpayer(ROOT).unwrap().unwrap().branches[0].facts.email.clone()
}

#[test]
fn a_filled_profile_box_is_changed_on_the_return_only_and_again() {
    let _serial = serial();
    let (_dir, app, tin) = app_with_taxpayer("juana@example.com");
    let opened = open(&app, &tin, 3);
    let key = email_key(&app);
    assert!(opened.profile_boxes.contains(&key));
    assert_eq!(opened.profile_values.get(&key).map(|v| v.to_lowercase()).as_deref(), Some("juana@example.com"));
    assert_eq!(opened.profile_facts.get(&key).map(String::as_str), Some("email"));
    assert!(opened.profile_edited.is_empty());

    let unlocked = [key.clone()];
    let first = app.fill(FORM, &opened.draft, &text(&key, "other@example.com"), &unlocked).unwrap();
    assert!(!first.rejected.contains_key(&key), "refused: {:?}", first.rejected.get(&key));
    assert_eq!(shown(&first.values, &key), "other@example.com");
    assert!(first.profile_blank.is_empty(), "nothing goes back to the profile");
    // The second edit of the same box is kept too.
    let second = app.fill(FORM, &first.draft, &text(&key, "third@example.com"), &unlocked).unwrap();
    assert!(!second.rejected.contains_key(&key), "refused: {:?}", second.rejected.get(&key));
    assert_eq!(shown(&second.values, &key), "third@example.com");

    app.save(FORM, &second.draft).unwrap();
    assert_eq!(profile_email(&app), "juana@example.com", "the profile is unchanged");
    let reopened = open(&app, &tin, 3);
    assert_eq!(shown(&reopened.values, &key), "third@example.com");
    assert!(reopened.profile_edited.contains(&key));
    // Another return still prefills the profile's value.
    assert_eq!(shown(&open(&app, &tin, 2).values, &key), "juana@example.com");
}

#[test]
fn a_blank_profile_box_filled_on_a_return_is_not_written_back() {
    let _serial = serial();
    let (_dir, app, tin) = app_with_taxpayer("");
    let opened = open(&app, &tin, 3);
    let key = email_key(&app);
    assert!(opened.profile_boxes.contains(&key));
    assert_eq!(opened.profile_values.get(&key).map(String::as_str), Some(""));

    let filled = app.fill(FORM, &opened.draft, &text(&key, "juana@example.com"), &[]).unwrap();
    assert!(!filled.rejected.contains_key(&key), "refused: {:?}", filled.rejected.get(&key));
    app.save(FORM, &filled.draft).unwrap();
    assert_eq!(profile_email(&app), "", "the return's value stays on the return");

    // Edited again after the save, opened as the page opens it.
    let reopened = open(&app, &tin, 3);
    assert!(reopened.profile_edited.contains(&key));
    let again = app.fill(FORM, &reopened.draft, &text(&key, "santos@example.com"), std::slice::from_ref(&key)).unwrap();
    assert!(!again.rejected.contains_key(&key), "refused: {:?}", again.rejected.get(&key));
    assert_eq!(shown(&again.values, &key), "santos@example.com");
}

#[test]
fn save_to_profile_too_writes_the_profile_and_the_box_stays_editable() {
    let _serial = serial();
    let (_dir, app, tin) = app_with_taxpayer("");
    let opened = open(&app, &tin, 3);
    let key = email_key(&app);
    let unlocked = [key.clone()];
    let filled = app.fill(FORM, &opened.draft, &text(&key, "juana@example.com"), &unlocked).unwrap();
    let fact = opened.profile_facts.get(&key).expect("the email box shows a profile fact");
    app.save_box_to_profile(&tin, fact, "juana@example.com").unwrap();
    assert_eq!(profile_email(&app), "juana@example.com");

    // The profile now has the value; the box still takes edits on this return.
    let again = app.fill(FORM, &filled.draft, &text(&key, "other@example.com"), &unlocked).unwrap();
    assert!(!again.rejected.contains_key(&key), "refused: {:?}", again.rejected.get(&key));
    assert_eq!(profile_email(&app), "juana@example.com", "only the explicit save writes the profile");
    // Later returns prefill what was saved.
    let next = open(&app, &tin, 2);
    assert_eq!(shown(&next.values, &key), "juana@example.com");
    assert_eq!(next.profile_values.get(&key).map(|v| v.to_lowercase()).as_deref(), Some("juana@example.com"));

    app.save_box_to_profile(&tin, "line_of_business", " 47190 Retail sale ").unwrap();
    assert_eq!(app.profile_details(ROOT, 2026).unwrap().line_of_business, "47190 Retail sale");
    assert!(app.save_box_to_profile(&tin, "full_name", "Someone").is_err());
}

#[test]
fn use_profile_value_puts_the_profiles_value_back_on_one_box() {
    let _serial = serial();
    let (_dir, app, tin) = app_with_taxpayer("juana@example.com");
    let opened = open(&app, &tin, 3);
    let key = email_key(&app);
    let filled = app.fill(FORM, &opened.draft, &text(&key, "other@example.com"), std::slice::from_ref(&key)).unwrap();
    let back = app.use_profile_values(FORM, 2026, 3, &filled.draft, std::slice::from_ref(&key)).unwrap();
    assert_eq!(shown(&back.values, &key), "juana@example.com");
    app.save(FORM, &back.draft).unwrap();
    let reopened = open(&app, &tin, 3);
    assert_eq!(shown(&reopened.values, &key), "juana@example.com");
    assert!(!reopened.profile_edited.contains(&key), "no longer the filer's edit");
}

#[test]
fn a_birth_date_is_set_kept_and_cleared() {
    let _serial = serial();
    let (_dir, app, _tin) = app_with_taxpayer("juana@example.com");
    let set = TaxpayerUpdate { birth_date: Some(Some("1991-02-03".into())), ..TaxpayerUpdate::default() };
    assert_eq!(app.update_taxpayer(ROOT, &set).unwrap().birth_date.as_deref(), Some("1991-02-03"));
    let rename = TaxpayerUpdate { name: Some("Juana P. Santos".into()), ..TaxpayerUpdate::default() };
    assert_eq!(app.update_taxpayer(ROOT, &rename).unwrap().birth_date.as_deref(), Some("1991-02-03"));
    let clear = TaxpayerUpdate { birth_date: Some(None), ..TaxpayerUpdate::default() };
    assert_eq!(app.update_taxpayer(ROOT, &clear).unwrap().birth_date, None);
    let bad = TaxpayerUpdate { birth_date: Some(Some("02/03/1991".into())), ..TaxpayerUpdate::default() };
    assert!(app.update_taxpayer(ROOT, &bad).is_err());
}

#[test]
fn business_and_filing_facts_round_trip_and_prefill_the_forms() {
    let _serial = serial();
    let (_dir, app, tin) = app_with_taxpayer("juana@example.com");
    let blank = app.profile_details(ROOT, 2026).unwrap();
    assert_eq!(blank, ProfileDetails { election_year: 2026, ..ProfileDetails::default() });
    let details = ProfileDetails {
        line_of_business: "47190 Retail sale".into(),
        business_start_date: Some("2020-03-01".into()),
        eopt_tier: Some("Micro".into()),
        government_withholding: false,
        tax_classification: Some("SelfEmployed".into()),
        election_year: 2026,
        income_tax_election: Some("EightPercent".into()),
        has_employees: true,
        withholds_compensation: true,
        withholds_expanded: false,
        withholds_final: false,
        vat_registered: false,
        dormant: false,
        single_employer: false,
    };
    assert_eq!(app.update_profile_details(ROOT, &details).unwrap(), details);
    assert_eq!(app.profile_details(ROOT, 2026).unwrap(), details);
    assert_eq!(app.profile_details(ROOT, 2025).unwrap().income_tax_election, None);
    assert!(app.update_profile_details(ROOT, &ProfileDetails { eopt_tier: Some("Huge".into()), ..details.clone() }).is_err());
    assert!(app.update_profile_details(ROOT, &ProfileDetails { business_start_date: Some("2020-13-01".into()), ..details.clone() }).is_err());

    let opened = open(&app, &tin, 3);
    let line = opened
        .profile_facts
        .iter()
        .find(|(_, fact)| fact.as_str() == "line_of_business")
        .map(|(key, _)| key.clone())
        .expect("1701Q has a line of business box");
    assert!(shown(&opened.values, &line).contains("retail sale"), "{:?}", opened.values.get(&line));
}
