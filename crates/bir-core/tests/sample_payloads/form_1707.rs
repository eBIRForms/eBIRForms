use crate::sample_payloads::penalty_items::{self, Deadline, day};
use crate::{check_sample, dummy_profile};
use bir_core::forms::form_1707::{
    Form1707Atc, Form1707Draft, Form1707Expense, Form1707Party, Form1707Shares,
    Form1707TransactionType,
};
use bir_core::forms::queueable::QueueableForm as _;

fn sample_1707() -> Form1707Draft {
    let mut draft = Form1707Draft::new_from_profile(&dummy_profile("1707", "Individual"), 2025, 1);
    draft.transaction_month = 6;
    draft.transaction_day = 30;
    draft.transaction_year = 2025;
    draft.number_of_attached_sheets = 1;
    draft.atc = Some(Form1707Atc::Individual);
    draft.sellers = vec![
        Form1707Party {
            name: "Sample Dummy Taxpayer".into(),
            address: "123 Sample Street, Quezon City".into(),
            tin: "123456788000".into(),
        },
        Form1707Party {
            name: "Juan Sample-Co".into(),
            address: "9 Example Rd., Pasig".into(),
            tin: "11122233300000".into(),
        },
    ];
    draft.buyers = vec![Form1707Party {
        name: "Example Holdings Inc.".into(),
        address: "45 Example Ave., Makati City".into(),
        tin: "444555666000".into(),
    }];
    draft.tax_relief = Some("Tax treaty relief".into());
    draft.set_transaction_type(Form1707TransactionType::CashSale);
    draft.shares = vec![
        Form1707Shares {
            corporation: "Sample Corp".into(),
            number_of_shares: Some(1_000.0),
            certificate_number: "1001".into(),
            selling_price: 250_000.005,
        },
        Form1707Shares {
            corporation: "Example Mining Inc".into(),
            number_of_shares: Some(12_345.678_5),
            certificate_number: "2002".into(),
            selling_price: 1_234_567.89,
        },
        Form1707Shares {
            corporation: "Third Sample Co".into(),
            number_of_shares: Some(10.0),
            certificate_number: "3003".into(),
            selling_price: 999.995,
        },
    ];
    draft.expenses = vec![
        Form1707Expense {
            particulars: "Acquisition cost".into(),
            amount: 900_000.0,
        },
        Form1707Expense {
            particulars: "Broker fees".into(),
            amount: 12_345.675,
        },
    ];
    draft.surcharge = 25.5;
    draft.interest = 10.005;
    draft.keep_entered_penalties();
    draft.recompute();
    draft
}

#[test]
fn form_1707_sample_payload_is_current() {
    let draft = sample_1707();
    let payload = draft
        .to_bir_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 1707 must validate: {errors:?}"));
    check_sample(
        "1707-06302025",
        "1707-v2021",
        &payload,
        "faa33104afa6c1a4e57e7a566193839c4c7d8c82da875e9bde1d4a5ce45ab53f",
    );
}

/// Schedules 2 and 3 past row D: the "More" pop-ups. The layout is the
/// generated one extended by `Form1707Draft::official_layout`.
fn sample_1707_with_popups() -> Form1707Draft {
    let mut draft = sample_1707();
    draft.transaction_month = 7;
    draft.transaction_day = 15;
    for (name, shares, cert, price) in [
        ("Fourth Corp", 40.0, "4004", 4_000.005),
        ("Fifth Co", 50.5, "5005", 5_000.5),
        ("Sixth Co", 1_234.567_8, "6006", 6_000.125),
    ] {
        draft.shares.push(Form1707Shares {
            corporation: name.into(),
            number_of_shares: Some(shares),
            certificate_number: cert.into(),
            selling_price: price,
        });
    }
    for (particulars, amount) in [
        ("Docs fees", 100.0),
        ("Notary", 200.5),
        ("Transfer tax", 300.255),
    ] {
        draft.expenses.push(Form1707Expense {
            particulars: particulars.into(),
            amount,
        });
    }
    draft.recompute();
    draft
}

#[test]
fn form_1707_sample_with_popups_is_current() {
    let draft = sample_1707_with_popups();
    let payload = draft
        .to_bir_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 1707 must validate: {errors:?}"));
    let layout = draft.official_layout().expect("extended layout");
    crate::check_sample_with_layout(
        "1707-07152025",
        &layout,
        &payload,
        "2f6c3b925d685656cb18ca41e064fb433cd5168e8abe29a2143a00b42ae394ef",
    );
}

/// More than ten rows in both schedules: the pop-ups have no row limit, so
/// every row past row D is written as D.1, D.2, … up to D.11 / D.9.
fn sample_1707_many_rows() -> Form1707Draft {
    let mut draft = sample_1707();
    draft.transaction_month = 8;
    draft.transaction_day = 29;
    for k in 4..=14u32 {
        draft.shares.push(Form1707Shares {
            corporation: format!("Row {k} Corp"),
            number_of_shares: Some(f64::from(k) * 10.5),
            certificate_number: format!("{k}{k}"),
            selling_price: f64::from(k) * 1_000.125,
        });
    }
    for k in 3..=12u32 {
        draft.expenses.push(Form1707Expense {
            particulars: format!("Expense {k}"),
            amount: f64::from(k) * 100.5,
        });
    }
    draft.recompute();
    draft
}

#[test]
fn form_1707_sample_with_many_rows_is_current() {
    let draft = sample_1707_many_rows();
    assert_eq!((draft.shares.len(), draft.expenses.len()), (14, 12));
    let payload = draft
        .to_bir_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 1707 must validate: {errors:?}"));
    let layout = draft.official_layout().expect("extended layout");
    crate::check_sample_with_layout(
        "1707-08292025",
        &layout,
        &payload,
        "eadac7ee0cd2ac30df5e5d7d02bd59553b43ccbd3f791a1ad4892c2a663b3908",
    );
}

/// Event-based: the calendar has no 1707 deadline.
#[test]
fn form_1707_penalty_items_follow_the_shared_model() {
    penalty_items::check(
        sample_1707,
        Deadline::None {
            assumed: day(2025, 12, 31),
        },
    );
}

/// Every sample draft above, for the agent-form probe (`agent_forms`).
pub(crate) fn agent_seeds() -> Vec<serde_json::Value> {
    vec![
        serde_json::to_value(sample_1707()).expect("sample serializes"),
        serde_json::to_value(sample_1707_with_popups()).expect("sample serializes"),
        serde_json::to_value(sample_1707_many_rows()).expect("sample serializes"),
    ]
}
