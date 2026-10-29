use crate::dummy_profile;
use crate::sample_payloads::penalty_items::{self, Deadline, day};
use bir_core::forms::form_1702mx::{
    Form1702MXDeductionMethod, Form1702MXDraft, PercentInput, WholePeso, WholePesoInput,
};
use bir_core::forms::queueable::QueueableForm as _;

fn peso(value: i64) -> WholePesoInput {
    WholePesoInput::from_amount(WholePeso(value))
}

/// A calendar 2025 return with exempt, special-rate and regular income,
/// itemized deductions, a NOLCO row, MCIT carried over and tax credits.
pub(crate) fn sample_1702mx() -> Form1702MXDraft {
    let mut d =
        Form1702MXDraft::new_from_profile(&dummy_profile("1702MXv2018C", "Corporation"), 2025);
    d.incorporation_date = "01/15/2015".into();
    d.deduction_method = Form1702MXDeductionMethod::Itemized;
    d.relief_details.investment_promotion_agency = ["Peza".into(), "Boi".into(), String::new()];
    d.relief_details.legal_basis = ["Ra 7916".into(), "Eo 226".into(), String::new()];
    d.relief_details.registered_activity = ["Export mfg".into(), "Tourism".into(), String::new()];
    d.relief_details.effectivity_from = ["01/01/2020".into(), "01/01/2021".into(), String::new()];
    d.relief_details.effectivity_until = ["12/31/2026".into(), "12/31/2027".into(), String::new()];
    d.relief_basis.special_tax_rate = PercentInput::from_hundredths(500);

    let s2 = &mut d.schedule_2.items;
    s2[0].exempt = peso(1_000_000);
    s2[0].special = peso(2_000_000);
    s2[0].regular = peso(5_000_000);
    s2[1].exempt = peso(100_000);
    s2[1].special = peso(500_000);
    s2[1].regular = peso(2_000_000);
    s2[3].regular = peso(25_000);
    s2[5].regular = peso(150_000);
    d.schedule_2.item_14_special_rate = PercentInput::from_hundredths(500);
    d.schedule_2.item_14_regular_rate = PercentInput::from_hundredths(2_500);
    s2[15].special = peso(1_000);
    s2[17].regular = peso(63_000);

    let s5 = &mut d.schedule_5.amounts;
    s5[0].exempt = peso(50_000);
    s5[0].special = peso(100_000);
    s5[0].regular = peso(800_000);
    s5[2].regular = peso(120_500);
    s5[19].regular = peso(10_000);
    d.schedule_5.other_descriptions_17d_to_17i[0] = "Training costs".into();

    d.schedule_6.rows[0].description = "Pension fund".into();
    d.schedule_6.rows[0].legal_basis = "Ra 4917".into();
    d.schedule_6.rows[0].amounts.exempt = peso(5_000);

    let row = &mut d.schedule_7_1.rows[0];
    row.year_incurred = "2022".into();
    row.amount = peso(300_000);
    row.applied_previous_years = peso(100_000);
    row.applied_current_year = peso(50_000);

    let mcit = &mut d.schedule_9.rows[0];
    mcit.year = "2023".into();
    mcit.normal_income_tax = peso(10_000);
    mcit.mcit = peso(30_000);
    mcit.applied_current_year = peso(5_000);

    let s3 = &mut d.schedule_3.items_20_to_33;
    s3[0].regular = peso(50_000);
    s3[2].special = peso(3_000);
    s3[4].regular = peso(20_000);

    d.schedule_4.items[0].exempt = peso(90_000);
    d.schedule_4.items[0].special = peso(30_000);
    d.schedule_4.items[1].exempt = peso(1_000);

    d.part_ii.item_17_surcharge = peso(1_000);
    d.keep_entered_penalties();
    d.recompute();

    // Schedule 10 reconciles to Schedule 2 Item 13 per column.
    for column in 0..3 {
        let value = match column {
            0 => d.schedule_2.items[12].exempt.clone(),
            1 => d.schedule_2.items[12].special.clone(),
            _ => d.schedule_2.items[12].regular.clone(),
        };
        let target = &mut d.schedule_10.items[0];
        match column {
            0 => target.exempt = value,
            1 => target.special = value,
            _ => target.regular = value,
        }
    }
    d.recompute();
    d
}

#[test]
fn form_1702mx_sample_payload_is_current() {
    let payload = sample_1702mx()
        .to_official_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 1702MX must validate: {errors:?}"));
    crate::check_sample(
        "1702MX-1225",
        "1702mx-v2018c",
        &payload,
        "1df8e48a01967ab74814292fb0c8988bdcf44caf72a12897551102c24e2929c6",
    );
}

/// A fiscal, amended return under the optional standard deduction with a
/// special-rate net operating loss, another ATC and an overpayment carried
/// over.
pub(crate) fn sample_1702mx_fiscal() -> Form1702MXDraft {
    use bir_core::forms::form_1702mx::{Form1702MXFilingBasis, Form1702MXOverpaymentDisposition};
    let mut d =
        Form1702MXDraft::new_from_profile(&dummy_profile("1702MXv2018C", "Corporation"), 2025);
    d.filing_basis = Form1702MXFilingBasis::Fiscal;
    d.month = 6;
    d.is_amended = true;
    d.incorporation_date = "03/10/2023".into();
    d.atc.other_selected = true;
    d.atc.other_code = "IC030".into();
    d.deduction_method = Form1702MXDeductionMethod::OptionalStandard;
    d.relief_details.investment_promotion_agency[1] = "Boi".into();
    d.relief_details.legal_basis[1] = "Eo 226".into();
    d.relief_details.registered_activity[1] = "Tourism".into();
    d.relief_details.effectivity_from[1] = "01/01/2021".into();
    d.relief_details.effectivity_until[1] = "12/31/2027".into();
    d.relief_basis.special_tax_rate = PercentInput::from_hundredths(750);
    let s2 = &mut d.schedule_2.items;
    s2[0].special = peso(1_000_000);
    s2[1].special = peso(300_000);
    s2[0].regular = peso(2_000_000);
    s2[1].regular = peso(1_500_000);
    d.schedule_2.item_14_special_rate = PercentInput::from_hundredths(750);
    d.schedule_2.item_14_regular_rate = PercentInput::from_hundredths(2_500);
    d.schedule_5.amounts[0].special = peso(900_000);
    let s3 = &mut d.schedule_3.items_20_to_33;
    s3[0].special = peso(100_000);
    s3[0].regular = peso(100_000);
    s3[7].regular = peso(10_000);
    d.recompute();
    d.schedule_10.items[0].special = d.schedule_2.items[12].special.clone();
    d.schedule_10.items[0].regular = d.schedule_2.items[12].regular.clone();
    d.part_ii.overpayment_disposition = Some(Form1702MXOverpaymentDisposition::CarryOver);
    d.recompute();
    d
}

#[test]
fn form_1702mx_fiscal_sample_payload_is_current() {
    let payload = sample_1702mx_fiscal()
        .to_official_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy fiscal 1702MX must validate: {errors:?}"));
    crate::check_sample(
        "1702MX-0625-fiscal",
        "1702mx-v2018c",
        &payload,
        "93d6ee49a729f9208ca162327c71a94ebbbb8632ef9bae9f753f86fb6dd5e7ea",
    );
}

fn special_row(
    description: &str,
    legal_basis: &str,
    column: usize,
    value: i64,
) -> bir_core::forms::form_1702mx::Form1702MXSpecialDeductionRow {
    let mut row = bir_core::forms::form_1702mx::Form1702MXSpecialDeductionRow {
        description: description.into(),
        legal_basis: legal_basis.into(),
        ..Default::default()
    };
    match column {
        0 => row.amounts.exempt = peso(value),
        1 => row.amounts.special = peso(value),
        _ => row.amounts.regular = peso(value),
    }
    row
}

/// Schedule 6 with all four page rows and seven Item 4 "(Add more...)"
/// rows: the first is the Item 4 entry the link transfers, the rest are
/// added with Add Row (`AddRow_modalTablePg3Sc6` has no limit).
pub(crate) fn sample_1702mx_schedule_6_rows() -> Form1702MXDraft {
    let mut d = sample_1702mx();
    d.schedule_6.rows[1] = special_row("Fringe", "Ra 8424", 2, 20_000);
    d.schedule_6.rows[2] = special_row("Rd cost", "Ra 9999", 2, 15_000);
    d.schedule_6.rows[3] = special_row("Seminar", "Rr 2-98", 2, 4_000);
    d.schedule_6.more_rows = vec![d.schedule_6.rows[3].clone()];
    for n in 2..=7i64 {
        d.schedule_6.more_rows.push(special_row(
            &format!("Item {n}"),
            &format!("Rr {n}"),
            ((n % 2) * 2) as usize,
            1_000 * n + 250,
        ));
    }
    // Schedule 4 Item 2C: Schedule 6 now has regular-rate deductions.
    d.schedule_4.items[1].regular = peso(2_000);
    d.recompute();
    for column in 0..3 {
        let item_13 = &d.schedule_2.items[12];
        let value = [&item_13.exempt, &item_13.special, &item_13.regular][column].clone();
        let target = &mut d.schedule_10.items[0];
        *[&mut target.exempt, &mut target.special, &mut target.regular][column] = value;
    }
    d.recompute();
    d
}

#[test]
fn form_1702mx_schedule_6_rows_sample_payload_is_current() {
    let draft = sample_1702mx_schedule_6_rows();
    assert_eq!(draft.schedule_6.rows[3].description, "OTHERS");
    assert_eq!(draft.schedule_6.more_rows.len(), 7);
    let payload = draft
        .to_official_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 1702MX must validate: {errors:?}"));
    let layout = draft.official_layout().expect("extended layout");
    crate::check_sample_with_layout(
        "1702MX-1225-sched6",
        &layout,
        &payload,
        "f20652d58465bd2ca969ed947c33e5a9e801e1a165374605b9664372827aba72",
    );
}

/// Calendar year 2025: due 15 April 2026.
#[test]
fn form_1702mx_penalty_items_follow_the_shared_model() {
    penalty_items::check(sample_1702mx, Deadline::Calendar(day(2026, 4, 15)));
}

/// Every sample draft above, for the agent-form probe (`agent_forms`).
pub(crate) fn agent_seeds() -> Vec<serde_json::Value> {
    vec![
        serde_json::to_value(sample_1702mx()).expect("sample serializes"),
        serde_json::to_value(sample_1702mx_fiscal()).expect("sample serializes"),
        serde_json::to_value(sample_1702mx_schedule_6_rows()).expect("sample serializes"),
    ]
}
