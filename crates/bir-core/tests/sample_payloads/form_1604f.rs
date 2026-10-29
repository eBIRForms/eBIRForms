use crate::{check_sample, dummy_profile};
use bir_core::forms::form_1604f::{Form1604fAgentCategory, Form1604fDraft, Form1604fRemittance};

fn remittance(date: &str, reference: &str, tax: f64, penalty: f64) -> Form1604fRemittance {
    Form1604fRemittance {
        date: date.to_string(),
        reference: reference.to_string(),
        taxes_withheld: tax,
        penalties: penalty,
        total_remitted: 0.0,
    }
}

fn sample_1604f() -> Form1604fDraft {
    let mut draft = Form1604fDraft::new_from_profile(&dummy_profile("1604F", "Corporation"), 2025);
    draft.agent_category = Form1604fAgentCategory::Private;
    draft.tax_relief = true;
    draft.tax_relief_details = "Rp-us treaty".to_string();
    draft.schedules[0][0] = remittance("04/25/2025", "tra-0001", 12_345.675, 100.005);
    draft.schedules[0][1] = remittance("07/25/2025", "eror 0002", 999.995, 0.0);
    draft.schedules[0][3] = remittance("01/26/2026", "Tra-0004", 250_000.0, 1_234.5);
    draft.schedules[1][3] = remittance("01/30/2026", "ear-4", 5_000.0, 0.0);
    draft.schedules[2][2] = remittance("10/30/2025", "x3", 1.5, 2.25);
    draft.recompute();
    draft
}

#[test]
fn form_1604f_sample_payload_is_current() {
    let draft = sample_1604f();
    let payload = draft
        .to_bir_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 1604F must validate: {errors:?}"));
    check_sample(
        "1604F-2025",
        "1604f-v2018",
        &payload,
        "d9844725c4ba9214dbb6bc93185c7031f8d0f5bfbf458551a052617951455861",
    );
}

/// Every sample draft above, for the agent-form probe (`agent_forms`).
pub(crate) fn agent_seeds() -> Vec<serde_json::Value> {
    // Every quarter remitted (PR 6): the sample dates five of the twelve
    // schedule rows, so probing could not see the other rows' Date of
    // Remittance keep a whole MM/DD/YYYY date.
    let mut every_quarter = sample_1604f();
    for (s, schedule) in every_quarter.schedules.iter_mut().enumerate() {
        for (q, row) in schedule.iter_mut().enumerate() {
            let n = (s * 4 + q) as f64 + 1.0;
            *row = remittance(
                &format!("{:02}/25/2025", q * 3 + 3),
                &format!("tra-{s}-{q}"),
                100.0 * n,
                n,
            );
        }
    }
    every_quarter.recompute();
    vec![
        serde_json::to_value(sample_1604f()).expect("sample serializes"),
        serde_json::to_value(every_quarter).expect("every-quarter sample serializes"),
    ]
}
