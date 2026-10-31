//! Put the demo taxpayer in the database `BUWIZ_DATABASE_PATH` names (key from
//! `BUWIZ_DB_KEY`), so tools that launch the app have a return to open.
//!
//! `BUWIZ_DATABASE_PATH=/tmp/x.db BUWIZ_DB_KEY=<64 hex> cargo run -p bir-app-core --example seed_demo`
//!
//! `-- --full` adds what screenshots need: the forms Juana files (so the
//! calendar has deadlines), drafts, a saved payee, and five queued 1601C
//! returns left Queued, Sending, Failed (the relay's `tin_not_active`) and
//! Sent (with a demo receipt), and one moved deadline (1701Q Q3 2026, due
//! Nov 23 instead of BIR's Nov 16) so the Calendar shows an overridden row.

use std::collections::BTreeMap;

use bir_app_core::{BirApp, BranchFacts, FieldValue, NewTaxpayer, OpenReturn, Payee, SendStage};

const TIN_ROOT: &str = "000123453";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let full = std::env::args().any(|a| a == "--full");
    let app = BirApp::open_live()?;
    let taxpayer = match app.get_taxpayer(TIN_ROOT)? {
        Some(t) => {
            println!("demo taxpayer already there");
            if !full {
                return Ok(());
            }
            t
        }
        None => {
            let t = app.create_taxpayer(&NewTaxpayer {
                tin_root: TIN_ROOT.into(),
                name: "Juana Santos".into(),
                taxpayer_type: "Individual".into(),
                birth_date: Some("1990-01-15".into()),
                head_office: BranchFacts {
                    rdo_code: "043".into(),
                    registered_address: "1 Sample Street, Pasig City".into(),
                    zip_code: "1600".into(),
                    phone: "09170000100".into(),
                    email: "juana@example.com".into(),
                },
            })?;
            println!("seeded {} {}", t.tin_display, t.name);
            t
        }
    };
    if full {
        seed_returns(&app, &taxpayer.branches[0].tin)?;
        let moved = app.move_deadline("1701Q", "2026-Q3", "2026-11-23", Some("Demo extension"))?;
        println!("moved 1701Q Q3 2026 to {}", moved.due.unwrap_or_default());
    }
    Ok(())
}

fn seed_returns(app: &BirApp, tin: &str) -> Result<(), Box<dyn std::error::Error>> {
    if !app.list_returns(bir_app_core::ReturnFilter::All)?.is_empty() {
        println!("returns already there");
        return Ok(());
    }
    for year in [2025i64, 2026] {
        app.call("set_yearly_forms", vec![tin.into(), year.into(), serde_json::json!(["1701Q", "2551Q", "1601C", "1701"])])?;
    }
    let open = |code: &str, year: i64, period: i64| app.open_return(&OpenReturn { tin: tin.to_owned(), form_code: code.into(), year, period, key: None });
    for (code, year, period) in [("2551Q", 2026, 3), ("1701Q", 2026, 2), ("1701Q", 2026, 3)] {
        let opened = open(code, year, period)?;
        app.save(code, &opened.draft)?;
    }
    app.save_payee(TIN_ROOT, &Payee { name: "Reyes Law Office".into(), tin: "000345675".into() })?;

    let layout = app.form_layout("1601C")?;
    let mut ids = Vec::new();
    for month in [6i64, 7, 8, 9, 10] {
        let opened = open("1601C", 2026, month)?;
        let mut edits = BTreeMap::new();
        edits.insert("frm1601c:TaxWithheld_1".to_string(), FieldValue::Tick(true));
        edits.insert("frm1601c:CatAgent_P".to_string(), FieldValue::Tick(true));
        edits.insert("frm1601c:txtTax25".to_string(), FieldValue::from("150.00"));
        for item in layout.parts.iter().flat_map(|p| p.items.iter()).filter(|i| i.no == "14") {
            for f in item.fields.iter().filter(|f| f.kind == "money" && !f.computed) {
                edits.insert(f.key.clone(), FieldValue::from("50000.00"));
            }
        }
        let filled = app.fill("1601C", &opened.draft, &edits, &[])?;
        let checked = app.validate("1601C", &filled.draft)?;
        let saved = app.save("1601C", &checked.draft)?;
        let draft = saved.draft.unwrap_or(checked.draft);
        ids.push(app.queue_return("1601C", &draft)?);
    }
    let in_two_minutes = (chrono::Utc::now() + chrono::Duration::minutes(2)).to_rfc3339();
    app.demo_set_send_stage(ids[0], &SendStage::Sent { at: None, receipt: None })?;
    app.demo_set_send_stage(
        ids[1],
        &SendStage::Failed {
            reason: "Not sent (nothing reached BIR): TIN 000123453 is not active on this account (tin_not_active)".into(),
            attempts: 1,
            next_retry_at: Some(in_two_minutes),
        },
    )?;
    app.demo_set_send_stage(ids[2], &SendStage::Sending)?;
    app.demo_set_send_stage(
        ids[4],
        &SendStage::NeedsAttention {
            reason: "The connection broke during the upload, so BIR may have the file. Check your email for BIR's confirmation before you send it again.".into(),
        },
    )?;
    println!(
        "seeded drafts and 1601C returns: sent {}, failed {}, sending {}, queued {}, needs attention {}",
        ids[0], ids[1], ids[2], ids[3], ids[4]
    );
    Ok(())
}
