use crate::sample_payloads::penalty_items::{self, Deadline, day};
use crate::{check_sample, dummy_profile};
use bir_core::forms::form_1707a::{Form1707AAtc, Form1707ADraft, Form1707ARow};
use bir_core::forms::queueable::QueueableForm as _;

fn row(date: &str, name: &str, selling: f64, cost: f64, tax_paid: f64) -> Form1707ARow {
    Form1707ARow {
        date: date.into(),
        corporation: name.into(),
        selling_price: selling,
        cost,
        gain_or_loss: 0.0,
        tax_paid,
    }
}

fn sample_1707a() -> Form1707ADraft {
    let mut draft =
        Form1707ADraft::new_from_profile(&dummy_profile("1707A", "Individual"), 2025, 1);
    draft.set_atc(Form1707AAtc::Individual);
    draft.year_end_year = 2025;
    draft.number_of_attached_sheets = 1;
    draft.tax_relief = Some("Treaty relief".into());
    draft.gains = vec![
        row(
            "02/15/2025",
            "Sample Corp",
            500_000.009,
            120_000.0,
            15_000.5,
        ),
        row(
            "06/30/2025",
            "Example Mining",
            1_234_567.891,
            1_000_000.105,
            0.0,
        ),
    ];
    draft.losses = vec![row("09/01/2025", "Loss Co", 50_000.0, 80_000.55, 0.0)];
    draft.surcharge = 25.5;
    draft.interest = 10.009;
    draft.keep_entered_penalties();
    draft.recompute();
    draft
}

#[test]
fn form_1707a_sample_payload_is_current() {
    let draft = sample_1707a();
    let payload = draft
        .to_bir_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 1707A must validate: {errors:?}"));
    check_sample(
        "1707A-12312025",
        "1707a-v2021",
        &payload,
        "6fd4be4d37c9c80a824544001c231ed7771f5056dc4105ab7e68067b37d48e0d",
    );
}

/// Schedules 1 and 2 past row 4: the "More" pop-ups. The layout is the generated one
/// extended by `Form1707ADraft::official_layout`.
fn sample_1707a_with_popup() -> Form1707ADraft {
    let mut draft = sample_1707a();
    draft.year_end_year = 2024;
    for row in draft.gains.iter_mut().chain(draft.losses.iter_mut()) {
        row.date = row.date.replace("/2025", "/2024");
    }
    draft.gains.extend([
        row("07/01/2024", "Third Co", 1_000.0, 500.0, 0.0),
        row("08/01/2024", "Fourth Co", 20_000.509, 15_000.0, 100.0),
        row("09/01/2024", "Fifth Co", 3_000.25, 1_000.0, 50.0),
        row("10/01/2024", "Sixth Co", 1_234_567.891, 1_000_000.0, 0.0),
    ]);
    draft.losses.extend([
        row("10/05/2024", "Loss Two", 10_000.0, 12_000.0, 0.0),
        row("11/01/2024", "Loss Three", 5_000.0, 5_000.5, 0.0),
        row("11/15/2024", "Loss Four", 1_000.0, 1_500.0, 0.0),
        row("12/01/2024", "Loss Five", 200.0, 300.25, 0.0),
    ]);
    draft.recompute();
    draft
}

#[test]
fn form_1707a_sample_with_popup_is_current() {
    let draft = sample_1707a_with_popup();
    let payload = draft
        .to_bir_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 1707A must validate: {errors:?}"));
    let layout = draft.official_layout().expect("extended layout");
    crate::check_sample_with_layout(
        "1707A-12312024",
        &layout,
        &payload,
        "a1ca2c78434fb131a887020bcf6b5f78c7b3e065bbeb22a887b2a98bb4d00191",
    );
}

/// More than ten rows in both schedules: the pop-ups have no row limit.
fn sample_1707a_many_rows() -> Form1707ADraft {
    let mut draft = sample_1707a();
    draft.year_end_year = 2023;
    for row in draft.gains.iter_mut().chain(draft.losses.iter_mut()) {
        row.date = row.date.replace("/2025", "/2023");
    }
    for k in 3..=14u32 {
        let month = (k - 3) % 12 + 1;
        draft.gains.push(row(
            &format!("{month:02}/10/2023"),
            &format!("Gain Co {k}"),
            f64::from(k) * 2_000.509,
            f64::from(k) * 1_000.0,
            f64::from(k) * 10.0,
        ));
    }
    for k in 2..=12u32 {
        let month = (k - 2) % 12 + 1;
        draft.losses.push(row(
            &format!("{month:02}/20/2023"),
            &format!("Loss Co {k}"),
            f64::from(k) * 500.0,
            f64::from(k) * 750.25,
            0.0,
        ));
    }
    draft.recompute();
    draft
}

#[test]
fn form_1707a_sample_with_many_rows_is_current() {
    let draft = sample_1707a_many_rows();
    assert_eq!((draft.gains.len(), draft.losses.len()), (14, 12));
    let payload = draft
        .to_bir_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 1707A must validate: {errors:?}"));
    let layout = draft.official_layout().expect("extended layout");
    crate::check_sample_with_layout(
        "1707A-12312023",
        &layout,
        &payload,
        "84d47ec4db11f76eda439d651a93b591bd42d3b09599b8b84b05ad1d8c264d56",
    );
}

/// Year ended 31 December 2025: due 15 April 2026.
#[test]
fn form_1707a_penalty_items_follow_the_shared_model() {
    penalty_items::check(sample_1707a, Deadline::Calendar(day(2026, 4, 15)));
}

/// Every sample draft above, for the agent-form probe (`agent_forms`).
pub(crate) fn agent_seeds() -> Vec<serde_json::Value> {
    vec![
        serde_json::to_value(sample_1707a()).expect("sample serializes"),
        serde_json::to_value(sample_1707a_with_popup()).expect("sample serializes"),
        serde_json::to_value(sample_1707a_many_rows()).expect("sample serializes"),
    ]
}
