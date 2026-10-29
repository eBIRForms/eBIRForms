use crate::{check_sample, dummy_profile};
use bir_core::forms::form_1604c::{Form1604cAgentCategory, Form1604cDraft, Form1604cRemittance};

fn month(
    date: &str,
    bank: &str,
    reference: &str,
    tax: f64,
    adjustment: f64,
    penalty: f64,
) -> Form1604cRemittance {
    Form1604cRemittance {
        date: date.to_string(),
        bank: bank.to_string(),
        reference: reference.to_string(),
        taxes_withheld: tax,
        adjustment,
        penalties: penalty,
        total_remitted: 0.0,
    }
}

fn sample_1604c() -> Form1604cDraft {
    let mut draft = Form1604cDraft::new_from_profile(&dummy_profile("1604C", "Corporation"), 2025);
    draft.agent_category = Form1604cAgentCategory::Private;
    draft.top_withholding_agent = true;
    draft.refunds_released = true;
    draft.refund_date = "03/15/2026".to_string();
    draft.overremittance = 4_321.555;
    draft.first_crediting_month = 2;
    draft.months[0] = month(
        "02/10/2025",
        "Sample bank-001",
        "tra1",
        10_000.005,
        0.0,
        25.5,
    );
    draft.months[1] = month("03/10/2025", "sample bank", "tra2", 8_000.5, -500.25, 0.0);
    draft.months[2] = month("04/10/2025", "Bank 3", "tra3", 100.0, -300.0, 0.0);
    draft.months[5] = month("07/10/2025", "Agency 6", "eROR-6", 999.995, 0.0, 0.005);
    draft.months[11] = month(
        "01/15/2026",
        "Bank Code 12",
        "TRA-12",
        12_345.675,
        100.0,
        0.0,
    );
    draft.recompute();
    draft
}

#[test]
fn form_1604c_sample_payload_is_current() {
    let draft = sample_1604c();
    let payload = draft
        .to_bir_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 1604C must validate: {errors:?}"));
    check_sample(
        "1604C-2025",
        "1604c-v2018",
        &payload,
        "72f81ba37b17df66502d3651700938ce71a47d834d2155dc9b65f72289c12766",
    );
}

/// Every sample draft above, for the agent-form probe (`agent_forms`).
pub(crate) fn agent_seeds() -> Vec<serde_json::Value> {
    // Every month remitted (PR 6): the sample dates five of Part II's
    // twelve rows, so probing could not see the other rows' Date of
    // Remittance keep a whole MM/DD/YYYY date.
    let mut every_month = sample_1604c();
    for (i, row) in every_month.months.iter_mut().enumerate() {
        let n = i as f64 + 1.0;
        *row = month(
            &format!("{:02}/10/2025", i + 1),
            &format!("Bank {}", i + 1),
            &format!("tra-{}", i + 1),
            1_000.0 * n,
            10.0 * n,
            n,
        );
    }
    every_month.recompute();
    vec![
        serde_json::to_value(sample_1604c()).expect("sample serializes"),
        serde_json::to_value(every_month).expect("every-month sample serializes"),
    ]
}
