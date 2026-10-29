use crate::sample_payloads::penalty_items::{self, Deadline, day};
use crate::{check_sample, dummy_profile};
use bir_core::forms::form_1800::{
    Form1800Donee, Form1800Draft, Form1800PersonalProperty, Form1800RealProperty,
};
use bir_core::forms::queueable::QueueableForm as _;

fn sample_1800() -> Form1800Draft {
    let mut draft = Form1800Draft::new_from_profile(&dummy_profile("1800", "Individual"), 2025, 1);
    draft.donation_month = 3;
    draft.donation_day = 10;
    draft.donation_year = 2025;
    draft.number_of_attached_sheets = 1;
    draft.donees = vec![
        Form1800Donee {
            name: "Maria Sample-Donee".into(),
            tin: "111222333000".into(),
        },
        Form1800Donee {
            name: "Pedro Example".into(),
            tin: "444555666".into(),
        },
    ];
    draft.tax_relief = Some("Special law relief".into());
    draft.personal_properties = vec![
        Form1800PersonalProperty {
            particulars: "Sedan, 2020 model".into(),
            fair_market_value: 750_000.005,
        },
        Form1800PersonalProperty {
            particulars: "Shares of Sample Corp".into(),
            fair_market_value: 1_234_567.894,
        },
    ];
    draft.real_properties = vec![Form1800RealProperty {
        title_number: "T-98765".into(),
        tax_declaration_number: "TD-0001".into(),
        location: "Lot 1, Sample Subd., Quezon City".into(),
        lot_or_improvement: "Lot".into(),
        classification: "RR".into(),
        area: "150".into(),
        fmv_per_tax_declaration: "900000".into(),
        fmv_per_zonal_value: "1200000".into(),
        fair_market_value: 1_200_000.0,
    }];
    draft.prior_gift_payments = 1_000.0;
    draft.surcharge = 25.5;
    draft.interest = 10.005;
    draft.keep_entered_penalties();
    draft.recompute();
    draft
}

#[test]
fn form_1800_sample_payload_is_current() {
    let draft = sample_1800();
    let payload = draft
        .to_bir_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 1800 must validate: {errors:?}"));
    check_sample(
        "1800-03102025",
        "1800-v2018",
        &payload,
        "d0af2bdd25f0b116bafe609bcbbc39ecf8b40fd07d9ee49f8f58c9b88abc06b3",
    );
}

/// More than five rows in Schedules A and B: `addScheduleA`,
/// `addScheduleB1` and `addScheduleB2` have no row limit, so rows 6 onwards
/// follow row 5 of their table.
fn sample_1800_many_rows() -> Form1800Draft {
    let mut draft = sample_1800();
    draft.donation_day = 11;
    for k in 3..=8u32 {
        draft.personal_properties.push(Form1800PersonalProperty {
            particulars: format!("Item {k} jewelry"),
            fair_market_value: f64::from(k) * 10_000.125,
        });
    }
    for k in 2..=7u32 {
        draft.real_properties.push(Form1800RealProperty {
            title_number: format!("T-{k}{k}{k}"),
            tax_declaration_number: format!("TD-00{k}"),
            location: format!("Lot {k}, Sample Subd., Pasig City"),
            lot_or_improvement: "Improvement".into(),
            classification: "CR".into(),
            area: format!("{}", k * 10),
            fmv_per_tax_declaration: format!("{}", k * 100_000),
            fmv_per_zonal_value: format!("{}", k * 120_000),
            fair_market_value: f64::from(k) * 120_000.0,
        });
    }
    draft.recompute();
    draft
}

#[test]
fn form_1800_sample_with_many_rows_is_current() {
    let draft = sample_1800_many_rows();
    assert_eq!(
        (draft.personal_properties.len(), draft.real_properties.len()),
        (8, 7)
    );
    let payload = draft
        .to_bir_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 1800 must validate: {errors:?}"));
    let layout = draft.official_layout().expect("extended layout");
    crate::check_sample_with_layout(
        "1800-03112025",
        &layout,
        &payload,
        "aa28d62fa9873c7a6522e44e7717bd76ff5141878f4347b5469d43646fdd16d4",
    );
}

/// Due 30 days after the donation (maintainer decision (1800/1801 penalties run
/// from the due date)): the penalties run from
/// that day; the calendar has no 1800 deadline.
#[test]
fn form_1800_penalty_items_follow_the_shared_model() {
    penalty_items::check(
        sample_1800,
        // Item 1 is 03/10/2025: due Wednesday, April 9, 2025.
        Deadline::Typed {
            due: day(2025, 4, 9),
        },
    );
}

/// Every sample draft above, for the agent-form probe (`agent_forms`).
pub(crate) fn agent_seeds() -> Vec<serde_json::Value> {
    vec![
        serde_json::to_value(sample_1800()).expect("sample serializes"),
        serde_json::to_value(sample_1800_many_rows()).expect("sample serializes"),
    ]
}
