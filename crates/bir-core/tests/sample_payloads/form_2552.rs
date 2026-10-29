use crate::sample_payloads::penalty_items::{self, Deadline, day};
use crate::{check_sample, dummy_profile, samples_dir};
use bir_core::forms::form_2552::{
    Form2552Draft, Form2552ExemptRow, Form2552NonTaxableRow, Form2552ShareRow, Form2552Transaction,
};
use bir_core::forms::queueable::QueueableForm as _;
use bir_core::official_xml;

fn share(date: &str, seller: &str, buyer: &str, shares: f64, base: f64) -> Form2552ShareRow {
    Form2552ShareRow {
        date: date.to_string(),
        seller: seller.to_string(),
        buyer: buyer.to_string(),
        issuing_corporation: "Sample Listed Corp".to_string(),
        number_of_shares: shares,
        tax_base: base,
        ..Default::default()
    }
}

fn sample_2552() -> Form2552Draft {
    let mut draft =
        Form2552Draft::new_from_profile(&dummy_profile("2552", "Corporation"), 2025, 3, 14);
    draft.is_amended = true;
    draft.number_of_attached_sheets = 1;
    draft.tax_relief = true;
    draft.tax_relief_specification = "Special law sample".to_string();
    draft.set_transaction(Form2552Transaction::LocalStockExchange);
    draft.schedule = vec![
        share(
            "03/14/2025",
            "Sample Seller One",
            "Sample Buyer One",
            1_000.0,
            1_234_567.891,
        ),
        share(
            "03/14/2025",
            "Sample Seller Two",
            "sample buyer two",
            500.5,
            999_999.995,
        ),
        share(
            "03/13/2025",
            "Sample Seller Three",
            "Sample Buyer Three",
            75.0,
            2_500.83,
        ),
    ];
    draft.exempt_transactions = vec![
        Form2552ExemptRow {
            classification: "Exempt sample one".to_string(),
            amount: 1_000.005,
        },
        Form2552ExemptRow {
            classification: "Exempt sample two".to_string(),
            amount: 250.0,
        },
    ];
    draft.non_taxable_transactions = vec![Form2552NonTaxableRow {
        date: "03/12/2025".to_string(),
        seller: "Sample Seller Six".to_string(),
        buyer: "Sample Buyer Six".to_string(),
        issuing_corporation: "Other Sample Corp".to_string(),
        number_of_shares: 42.0,
    }];
    draft.tax_paid_previous = 100.5;
    draft.creditable_tax_withheld = 1_000.25;
    draft.surcharge = 25.5;
    draft.interest = 10.0;
    draft.keep_entered_penalties();
    draft.recompute();
    draft
}

#[test]
fn form_2552_sample_payload_is_current() {
    let draft = sample_2552();
    let payload = draft
        .to_bir_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 2552 must validate: {errors:?}"));
    check_sample(
        "2552-03142025",
        "2552-v2018",
        &payload,
        "a14ed48bace1df2367a4c8d1896e5778d8a4780738fe7a7bcde2d622ab110a65",
    );
}

/// A primary offering with seven transactions: rows 5–7 go through the
/// official "(Add More...)" popup and row 5 shows OTHERS.
fn sample_2552_add_more() -> Form2552Draft {
    let mut draft =
        Form2552Draft::new_from_profile(&dummy_profile("2552", "Corporation"), 2025, 3, 12);
    draft.set_transaction(Form2552Transaction::PrimaryOffering);
    draft.shares_sold = "1000000".to_string();
    draft.outstanding_shares = "5000000".to_string();
    let rows = [
        ("03/10/2025", "Issuer One", 1_000.0, 250_000.75, 4.0),
        ("03/10/2025", "Issuer Two", 2_000.0, 500_000.0, 2.0),
        ("03/11/2025", "Issuer Three", 300.5, 75_000.25, 1.0),
        ("03/11/2025", "Issuer Four", 400.0, 999.99, 4.0),
        ("03/12/2025", "Issuer Five", 50.0, 12_345.67, 2.5),
        ("03/12/2025", "Issuer Six", 60.9, 23_456.78, 4.5),
        ("03/12/2025", "Issuer Seven", 70.0, 888.88, 1.0),
    ];
    for (date, seller, shares, base, rate) in rows {
        draft.schedule.push(Form2552ShareRow {
            date: date.to_string(),
            seller: seller.to_string(),
            buyer: "Sample Public Buyer".to_string(),
            issuing_corporation: "Sample Listed Corp".to_string(),
            number_of_shares: shares,
            tax_base: base,
            tax_rate: rate,
            ..Default::default()
        });
    }
    draft.creditable_tax_withheld = 500.0;
    draft.recompute();
    draft
}

/// `check_sample` for a plaintext with popup rows: they are not part of the
/// fixed layout, so the layout is checked with them taken out.
#[test]
fn form_2552_add_more_sample_payload_is_current() {
    let stem = "2552-03122025";
    let plaintext = sample_2552_add_more()
        .to_bir_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 2552 must validate: {errors:?}"));
    let layout = official_xml::layout("2552-v2018").unwrap();
    let mut fixed = String::new();
    let mut rest = plaintext.as_str();
    while let Some(start) = rest.find("<div>frm2552:txtPg2Pt5Sch2_") {
        fixed.push_str(&rest[..start]);
        let end = start + rest[start..].find("</div>").unwrap() + "</div>".len();
        rest = &rest[end + layout.lead.len()..];
    }
    fixed.push_str(rest);
    assert_ne!(fixed, plaintext, "the sample has popup rows");
    official_xml::read(layout, &fixed).expect("the rest follows the official layout");

    let dir = samples_dir();
    let plain_path = dir.join(format!("{stem}.plain.xml"));
    let updating = std::env::var_os("UPDATE_SAMPLE_PAYLOADS").is_some();
    if updating {
        std::fs::write(&plain_path, &plaintext).unwrap();
    }
    let official = std::fs::read_to_string(dir.join(format!("{stem}.official.xml"))).unwrap();
    assert_eq!(
        plaintext, official,
        "{stem}: our plaintext differs from the official upload (saveEncryptedProfile)"
    );
    if !updating {
        assert_eq!(std::fs::read_to_string(&plain_path).unwrap(), plaintext);
    }
}

/// Event-based: the calendar has no 2552 deadline.
#[test]
fn form_2552_penalty_items_follow_the_shared_model() {
    penalty_items::check(
        sample_2552,
        Deadline::None {
            assumed: day(2025, 12, 31),
        },
    );
}

/// Every sample draft above, for the agent-form probe (`agent_forms`).
pub(crate) fn agent_seeds() -> Vec<serde_json::Value> {
    vec![
        serde_json::to_value(sample_2552()).expect("sample serializes"),
        serde_json::to_value(sample_2552_add_more()).expect("sample serializes"),
        serde_json::to_value(sample_2552_secondary()).expect("sample serializes"),
    ]
}

/// The add-more sample as a secondary offering (Item 11C, Schedule 3), so
/// the probe reaches Schedule 3's rows and popup too. `set_transaction`
/// would clear the rows, as the page's `kindOfTransaction` does.
fn sample_2552_secondary() -> Form2552Draft {
    let mut draft = sample_2552_add_more();
    draft.transaction = Form2552Transaction::SecondaryOffering;
    draft.recompute();
    draft
}
