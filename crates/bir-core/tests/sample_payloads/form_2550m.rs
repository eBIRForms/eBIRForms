use crate::sample_payloads::penalty_items::{self, Deadline, day};
use crate::{check_sample, dummy_profile, samples_dir};
use bir_core::forms::form_2550m::{
    Form2550MAdvancePaymentRow, Form2550MAllocation, Form2550MAmortizedRow,
    Form2550MCapitalGoodsRow, Form2550MDraft, Form2550MPurchase, Form2550MWithholdingRow,
};
use bir_core::forms::queueable::QueueableForm as _;
use bir_core::official_xml;

/// No Schedule 1 rows: the plaintext is exactly the fixed official layout.
fn sample_2550m() -> Form2550MDraft {
    let mut draft =
        Form2550MDraft::new_from_profile(&dummy_profile("2550M", "Corporation"), 2022, 6);
    draft.is_amended = true;
    draft.set_sales_to_government(250_000.005);
    draft.zero_rated_sales = 40_000.0;
    draft.exempt_sales = 12_345.67;
    draft.input_tax_carried_over = 1_500.25;
    draft.transitional_input_tax = 99.99;
    draft.set_purchase(Form2550MPurchase::DomesticGoods, 20_000.0);
    draft.set_purchase(Form2550MPurchase::ImportedGoods, 8_888.88);
    draft.set_purchase(Form2550MPurchase::DomesticServices, 3_333.33);
    draft.set_purchase(Form2550MPurchase::NonResidentServices, 1_000.0);
    draft.purchases_not_qualified = 500.0;
    draft.set_purchase(Form2550MPurchase::Others, 777.77);
    draft.schedule_4 = Some(Form2550MAllocation {
        direct_input_tax: 18_000.0,
        not_direct_input_tax: 1_200.0,
    });
    draft.schedule_5 = Some(Form2550MAllocation {
        direct_input_tax: 100.0,
        not_direct_input_tax: 300.0,
    });
    draft.vat_refund_claimed = 10.0;
    draft.vat_paid_previous = 1_000.0;
    draft.advance_payments = 200.0;
    draft.surcharge = 25.5;
    draft.interest = 10.0;
    draft.keep_entered_penalties();
    draft.recompute();
    draft
}

/// Schedule 1 rows (vatable sales per ATC), spliced ahead of the Schedule 1 totals.
fn sample_2550m_schedule_1() -> Form2550MDraft {
    let mut draft =
        Form2550MDraft::new_from_profile(&dummy_profile("2550M", "Corporation"), 2022, 7);
    draft.add_sales_atc("VT010").unwrap();
    draft.add_sales_atc("VB010").unwrap();
    draft.add_sales_atc("VS062").unwrap();
    // Popup order: VB010, VT010, VS062.
    draft.sales_schedule[0].amount = 2_345.67;
    draft.sales_schedule[1].amount = 100_000.005;
    draft.sales_schedule[2].amount = 55_555.55;
    draft.zero_rated_sales = 1_000.0;
    draft.input_tax_carried_over = 300.0;
    draft.set_purchase(Form2550MPurchase::DomesticGoods, 15_000.0);
    draft.other_credits = 50.0;
    draft.recompute();
    draft
}

#[test]
fn form_2550m_sample_payload_is_current() {
    let payload = sample_2550m()
        .to_bir_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 2550M must validate: {errors:?}"));
    check_sample(
        "2550M-062022",
        "2550m-v2007",
        &payload,
        "bb7ff9366bfb4c84729842a126861ecec0d3512f905c520d7e095d223d6ed09a",
    );
}

/// `check_sample` for a plaintext with schedule rows: they are not part of
/// the fixed layout, so the layout is checked with them taken out.
fn check_spliced_sample(stem: &str, plaintext: &str, _official_encrypt_sha256: &str) {
    let layout = official_xml::layout("2550m-v2007").unwrap();
    let known = layout.keys();
    let mut fixed = String::new();
    let mut rest = plaintext;
    let mut removed = 0;
    while let Some(start) = rest.find("<div>") {
        let end = start + rest[start..].find("</div>").unwrap() + "</div>".len();
        let key = rest[start + 5..].split('=').next().unwrap();
        if known.contains(key) {
            fixed.push_str(&rest[..end]);
            rest = &rest[end..];
        } else {
            fixed.push_str(&rest[..start]);
            rest = &rest[end + layout.lead.len()..];
            removed += 1;
        }
    }
    fixed.push_str(rest);
    assert!(removed > 0, "{stem} has schedule rows");
    official_xml::read(layout, &fixed).expect("the rest follows the official layout");

    let dir = samples_dir();
    let plain_path = dir.join(format!("{stem}.plain.xml"));
    let updating = std::env::var_os("UPDATE_SAMPLE_PAYLOADS").is_some();
    if updating {
        std::fs::write(&plain_path, plaintext).unwrap();
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

fn payload(draft: Form2550MDraft) -> String {
    draft
        .to_bir_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 2550M must validate: {errors:?}"))
}

#[test]
fn form_2550m_schedule_1_sample_payload_is_current() {
    check_spliced_sample(
        "2550M-072022",
        &payload(sample_2550m_schedule_1()),
        "db469f023104c6337570b4bf06b9b4a2f87b67a759d354f7acb4fd8f074c4225",
    );
}

/// Capital goods: Schedule 2 (one input tax edited) and Schedule 3 Parts A and B.
fn sample_2550m_capital_goods() -> Form2550MDraft {
    let mut draft =
        Form2550MDraft::new_from_profile(&dummy_profile("2550M", "Corporation"), 2022, 8);
    draft.add_sales_atc("VT010").unwrap();
    draft.sales_schedule[0].amount = 3_000_000.0;
    draft.schedule_2 = vec![
        Form2550MCapitalGoodsRow {
            date_purchased: "08/05/2022".into(),
            description: "Office laptop".into(),
            ..Default::default()
        },
        Form2550MCapitalGoodsRow {
            date_purchased: "08/20/2022".into(),
            description: "Delivery motorbike".into(),
            ..Default::default()
        },
    ];
    draft.set_schedule_2_amount(0, 85_000.55);
    draft.set_schedule_2_amount(1, 120_000.0);
    draft.schedule_2[1].input_tax = 14_000.0;
    draft.schedule_3a = vec![
        Form2550MAmortizedRow {
            date_purchased: "08/10/2022".into(),
            description: "Factory machine".into(),
            estimated_life: 120,
            recognized_life: 60,
            ..Default::default()
        },
        Form2550MAmortizedRow {
            date_purchased: "08/15/2022".into(),
            description: "Warehouse racks".into(),
            estimated_life: 60,
            recognized_life: 60,
            ..Default::default()
        },
    ];
    draft.set_schedule_3a_amount(0, 1_500_000.25);
    draft.set_schedule_3a_amount(1, 250_000.0);
    draft.schedule_3b = vec![Form2550MAmortizedRow {
        date_purchased: "03/01/2022".into(),
        description: "Generator set".into(),
        amount: 2_000_000.0,
        input_tax: 200_000.5,
        estimated_life: 120,
        recognized_life: 55,
        ..Default::default()
    }];
    draft.recompute();
    draft
}

#[test]
fn form_2550m_capital_goods_sample_payload_is_current() {
    check_spliced_sample(
        "2550M-082022",
        &payload(sample_2550m_capital_goods()),
        "968f32bebc3cd25ebe8445898a401891b7f0c0ef91fc883f9bb9796611172abb",
    );
}

/// Tax credits: Schedules 6, 7 and 8 (Schedule 8 needs sales to government).
fn sample_2550m_credits() -> Form2550MDraft {
    let mut draft =
        Form2550MDraft::new_from_profile(&dummy_profile("2550M", "Corporation"), 2022, 9);
    draft.add_sales_atc("VB010").unwrap();
    draft.sales_schedule[0].amount = 500_000.0;
    draft.set_sales_to_government(200_000.0);
    draft.schedule_6 = vec![
        Form2550MWithholdingRow {
            period_covered: "09/30/2022".into(),
            withholding_agent: "Sample Agent Corp".into(),
            income_payment: 100_000.0,
            total_withheld: 5_000.0,
            applied_current_month: 4_000.5,
        },
        Form2550MWithholdingRow {
            period_covered: "09/15/2022".into(),
            withholding_agent: "agent two".into(),
            income_payment: 50_000.0,
            total_withheld: 2_500.0,
            applied_current_month: 2_500.0,
        },
    ];
    draft.schedule_7 = vec![Form2550MAdvancePaymentRow {
        period_covered: "09/10/2022".into(),
        miller: "Sample Rice Mill".into(),
        taxpayer_name: "Sample Dummy".into(),
        or_number: "or-12345".into(),
        amount_paid: 3_000.0,
        applied_current_month: 1_500.25,
    }];
    draft.schedule_8 = vec![Form2550MWithholdingRow {
        period_covered: "09/30/2022".into(),
        withholding_agent: "Sample Gov Agency".into(),
        income_payment: 200_000.0,
        total_withheld: 10_000.0,
        applied_current_month: 10_000.0,
    }];
    draft.recompute();
    draft
}

#[test]
fn form_2550m_credits_sample_payload_is_current() {
    check_spliced_sample(
        "2550M-092022",
        &payload(sample_2550m_credits()),
        "98c8f3c08bb6f0a5db771793181015b9e07b1558b7f6e3c9ec63f203ed092aca",
    );
}

/// The calendar has no 2550-M rule.
#[test]
fn form_2550m_penalty_items_follow_the_shared_model() {
    penalty_items::check(
        sample_2550m,
        Deadline::None {
            assumed: day(2025, 12, 31),
        },
    );
}

/// Every sample draft above, for the agent-form probe (`agent_forms`).
pub(crate) fn agent_seeds() -> Vec<serde_json::Value> {
    vec![
        serde_json::to_value(sample_2550m()).expect("sample serializes"),
        serde_json::to_value(sample_2550m_schedule_1()).expect("sample serializes"),
        serde_json::to_value(sample_2550m_capital_goods()).expect("sample serializes"),
        serde_json::to_value(sample_2550m_credits()).expect("sample serializes"),
    ]
}
