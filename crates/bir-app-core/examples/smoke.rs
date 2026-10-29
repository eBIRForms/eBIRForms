//! "It runs": open a scratch database, add a fake taxpayer, open three
//! forms, fill, validate, save, queue one and print it.
//!
//! `cargo run -p bir-app-core --example smoke`. Uses `BIR_DB_KEY` when set,
//! else a throwaway all-zero test key; the database lives in a temp dir that
//! is deleted at the end.

use std::collections::BTreeMap;

use bir_app_core::{BirApp, BranchFacts, FieldValue, NewTaxpayer, OpenReturn, ReturnFilter};

const TEST_KEY: &str = "0000000000000000000000000000000000000000000000000000000000000000";
const TIN_ROOT: &str = "000123453";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var_os("BIR_DB_KEY").is_none() {
        // SAFETY: set before any other thread starts.
        unsafe { std::env::set_var("BIR_DB_KEY", TEST_KEY) };
    }
    let dir = tempfile::tempdir()?;
    let app = BirApp::open_at(dir.path().join("bir_data.db"))?;
    println!("database: {}", app.database_path().display());
    println!("functions: {}", app.function_names().len());

    let catalog = BirApp::form_catalog()?;
    println!(
        "forms: {} ({} ... {})",
        catalog.len(),
        catalog.first().map_or("", |form| form.code.as_str()),
        catalog.last().map_or("", |form| form.code.as_str()),
    );

    let taxpayer = app.create_taxpayer(&NewTaxpayer {
        tin_root: TIN_ROOT.into(),
        name: "Juana Dela Cruz".into(),
        taxpayer_type: "Individual".into(),
        birth_date: Some("1990-01-15".into()),
        head_office: BranchFacts {
            rdo_code: "043".into(),
            registered_address: "1 Sample Street, Pasig City".into(),
            zip_code: "1600".into(),
            phone: "09171234567".into(),
            email: "juana@example.com".into(),
        },
    })?;
    let tin = taxpayer.branches[0].tin.clone();
    println!(
        "taxpayer: {} {} ({} branch)",
        taxpayer.tin_display,
        taxpayer.name,
        taxpayer.branches.len()
    );

    let year = 2025;
    // The forms this branch files in the year, so the calendar has rows.
    app.call(
        "set_yearly_forms",
        vec![
            tin.as_str().into(),
            year.into(),
            serde_json::json!(["1701Q", "2551Q", "1601C"]),
        ],
    )?;
    let mut queued = None;
    for (form_code, period) in [("1701Q", 1), ("2551Q", 1), ("1601C", 1)] {
        let sample = app.sample_return(&tin, form_code)?;
        let opened = app.open_return(&OpenReturn {
            tin: tin.clone(),
            form_code: form_code.into(),
            year,
            period,
            key: None,
        })?;
        println!(
            "{form_code}: sample {} boxes; return {} {}/{} status {} with {} boxes",
            sample.values.len(),
            opened.form_code,
            opened.year,
            opened.period,
            opened.status,
            opened.values.len(),
        );

        // Fill one amount box the form leaves open.
        let key = fillable_amount(&app, form_code, &opened)?;
        let mut edits = BTreeMap::new();
        if form_code == "1601C" {
            // Item 3 "Any taxes withheld?" Yes, item 11 Private: the answers
            // its checks ask for, so this one can be queued.
            edits.insert("frm1601c:TaxWithheld_1".into(), FieldValue::Tick(true));
            edits.insert("frm1601c:CatAgent_P".into(), FieldValue::Tick(true));
            // Item 25, total taxes withheld.
            edits.insert("frm1601c:txtTax25".into(), FieldValue::from("150.00"));
        }
        if let Some(key) = &key {
            edits.insert(key.clone(), FieldValue::from("1000.00"));
        }
        let filled = app.fill(form_code, &opened.draft, &edits, &[])?;
        if let Some(key) = &key {
            println!(
                "  fill {key} = 1000.00 -> {:?}{}",
                filled.values.get(key),
                filled
                    .rejected
                    .get(key)
                    .map_or(String::new(), |why| format!(" (refused: {why})")),
            );
        }

        let validated = app.validate(form_code, &filled.draft)?;
        println!("  validate: {} message(s)", validated.errors.len());
        for error in validated.errors.iter().take(3) {
            println!("    {}: {}", error.field, error.message);
        }

        let saved = app.save(form_code, &validated.draft)?;
        println!(
            "  save: id {:?} status {:?}",
            saved.id,
            saved.status.as_deref()
        );
        let draft = saved.draft.clone().unwrap_or(validated.draft);

        if queued.is_none() {
            match app.queue_return(form_code, &draft) {
                Ok(id) => {
                    println!("  queue: draft {id} queued");
                    queued = Some((form_code, draft.clone()));
                }
                Err(error) => println!("  queue: refused ({error})"),
            }
        }

        let printed = app.print_html(form_code, &draft)?;
        println!(
            "  print: \"{}\", {} bytes of HTML",
            printed.title,
            printed.html.len()
        );
    }

    let queue = app.list_queue()?;
    println!("queue: {} return(s)", queue.len());
    for item in &queue {
        println!(
            "  {} {} {} {}",
            item.form_code, item.taxable_year, item.status, item.tin
        );
    }
    if let Some((form_code, draft)) = &queued {
        app.cancel_queued(form_code, draft)?;
        println!(
            "cancel: {form_code} back to draft, queue now {}",
            app.list_queue()?.len()
        );
        let id = app.queue_return(form_code, draft)?;
        println!("requeue: draft {id}, queue now {}", app.list_queue()?.len());
    }

    println!("drafts {year}: {}", app.list_drafts(&tin, year)?.len());
    println!(
        "returns: {} saved/filed",
        app.list_returns(ReturnFilter::All)?.len()
    );
    let deadlines = app.deadlines_between("2025-01-01", "2025-12-31")?;
    println!("deadlines 2025: {}", deadlines.len());
    let home = app.home(chrono::NaiveDate::from_ymd_opt(2025, 4, 1).expect("a date"))?;
    println!(
        "home: {} due, {} as-needed, {} recent",
        home.due.len(),
        home.as_needed.len(),
        home.recent.len()
    );
    println!("ok");
    Ok(())
}

/// The first amount box of the form that is neither locked nor computed.
fn fillable_amount(
    app: &BirApp,
    form_code: &str,
    opened: &bir_app_core::OpenedReturn,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let schema = app.form_schema(form_code)?;
    let mut keys = Vec::new();
    collect_amount_keys(&schema, &mut keys);
    Ok(keys
        .into_iter()
        .find(|key| opened.values.contains_key(key) && !opened.locks.contains_key(key)))
}

fn collect_amount_keys(value: &serde_json::Value, keys: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) => {
            // `t`: the box type; `c`: computed by the form.
            let is_input =
                map.get("t").and_then(|t| t.as_str()) == Some("money") && map.get("c").is_none();
            if is_input && let Some(key) = map.get("k").and_then(|k| k.as_str()) {
                keys.push(key.to_string());
            }
            for child in map.values() {
                collect_amount_keys(child, keys);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_amount_keys(item, keys);
            }
        }
        _ => {}
    }
}
