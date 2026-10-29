use crate::{check_sample, dummy_profile};
use bir_core::forms::form_1604e::{Form1604eAgentCategory, Form1604eDraft, Form1604eRemittance};

fn row(date: &str, bank: &str, reference: &str, tax: f64, penalty: f64) -> Form1604eRemittance {
    Form1604eRemittance {
        date: date.to_string(),
        bank: bank.to_string(),
        reference: reference.to_string(),
        taxes_withheld: tax,
        penalties: penalty,
        total_remitted: 0.0,
    }
}

fn sample_1604e() -> Form1604eDraft {
    let mut draft = Form1604eDraft::new_from_profile(&dummy_profile("1604E", "Corporation"), 2025);
    draft.agent_category = Form1604eAgentCategory::Private;
    draft.top_withholding_agent = Some(false);
    draft.schedule1[0] = row("04/30/2025", "SampleBank01", "traQ1", 12_345.675, 100.005);
    draft.schedule1[1] = row("07/31/2025", "Agency2", "eROR2", 999.995, 0.0);
    draft.schedule1[3] = row("01/30/2026", "bank4", "Ear4", 250_000.0, 0.0);
    draft.schedule2[0] = row("02/10/2025", "BankJan", "TRA0101", 1_000.5, 0.5);
    draft.schedule2[5] = row("07/10/2025", "bankjun", "tra06", 2_500.125, 0.0);
    draft.schedule2[11] = row("01/15/2026", "BANKDEC", "TRA12", 333.333, 12.0);
    draft.recompute();
    draft
}

#[test]
fn form_1604e_sample_payload_is_current() {
    let draft = sample_1604e();
    let payload = draft
        .to_bir_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 1604E must validate: {errors:?}"));
    check_sample(
        "1604E-2025",
        "1604e-v2018",
        &payload,
        "bfc67d4980b268e36403d03a7d8b373f4400f9e2c41975149b1274e9c43f62f7",
    );
}

/// Every sample draft above, for the agent-form probe (`agent_forms`).
pub(crate) fn agent_seeds() -> Vec<serde_json::Value> {
    // Every quarter and month remitted (PR 6): the sample dates three rows
    // of each schedule, so probing could not see the other rows' Date of
    // Remittance keep a whole MM/DD/YYYY date.
    let mut every_row = sample_1604e();
    for (s, schedule) in [&mut every_row.schedule1[..], &mut every_row.schedule2[..]]
        .into_iter()
        .enumerate()
    {
        for (i, remittance) in schedule.iter_mut().enumerate() {
            let n = i as f64 + 1.0;
            *remittance = row(
                &format!("{:02}/20/2025", i + 1),
                &format!("Bank {s}-{i}"),
                &format!("tra-{s}-{i}"),
                500.0 * n,
                5.0 * n,
            );
        }
    }
    every_row.recompute();
    vec![
        serde_json::to_value(sample_1604e()).expect("sample serializes"),
        serde_json::to_value(every_row).expect("every-row sample serializes"),
    ]
}
