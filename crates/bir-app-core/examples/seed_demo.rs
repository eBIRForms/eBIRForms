//! Put the demo taxpayer in the database `BIR_DATABASE_PATH` names (key from
//! `BIR_DB_KEY`), so tools that launch the app have a return to open.
//!
//! `BIR_DATABASE_PATH=/tmp/x.db BIR_DB_KEY=<64 hex> cargo run -p bir-app-core --example seed_demo`

use bir_app_core::{BirApp, BranchFacts, NewTaxpayer};

const TIN_ROOT: &str = "000123453";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let app = BirApp::open_live()?;
    if app.get_taxpayer(TIN_ROOT)?.is_some() {
        println!("demo taxpayer already there");
        return Ok(());
    }
    let taxpayer = app.create_taxpayer(&NewTaxpayer {
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
    println!("seeded {} {}", taxpayer.tin_display, taxpayer.name);
    Ok(())
}
