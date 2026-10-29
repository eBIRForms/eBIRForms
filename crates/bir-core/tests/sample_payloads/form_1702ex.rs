//! Profile fields are filled the way `loadBGData` does; the other steps are
//! raw filer input in form order.

use crate::{check_sample, dummy_profile};
use bir_core::forms::form_1702ex::{
    Form1702ExAtc, Form1702ExDeduction, Form1702ExDraft, Form1702ExOverpayment, Form1702ExRow,
    Form1702ExSpecialRow,
};

fn sample_1702ex() -> Form1702ExDraft {
    let mut draft =
        Form1702ExDraft::new_from_profile(&dummy_profile("1702EX", "Corporation"), 2025);
    draft.registered_name = "Sample Dummy Foundation Inc".to_string();
    draft.date_of_incorporation = "03/01/2010".to_string();
    draft.atc = Form1702ExAtc::IC011;
    draft.deduction = Form1702ExDeduction::Itemized;
    draft.legal_basis = "Sec 30 NIRC".to_string();
    draft.promotion_agency = "Sample Agency".to_string();
    draft.registered_activity = "REG-0001".to_string();
    draft.effectivity_from = "01/01/2010".to_string();
    draft.effectivity_to = "12/31/2030".to_string();
    draft.sales = 5_000_000.5;
    draft.sales_returns = 100_000.0;
    draft.cost_of_sales = 2_000_000.0;
    draft.other_income = 50_000.0;
    draft.cwt_q4 = 1_000.0;
    draft.regular_income_tax = 600_000.0;
    draft.special_allowable_relief = 15_000.0;
    draft.ordinary_items[1] = 10_000.0;
    draft.ordinary_items[12] = 500_000.0;
    draft.ordinary_items[16] = 20_000.0;
    draft.other_deductions = vec![Form1702ExRow {
        description: "Sample expense".to_string(),
        amount: 5_000.0,
    }];
    draft.special_rows = vec![Form1702ExSpecialRow {
        description: "Sample deduction".to_string(),
        legal_basis: "RA 0000".to_string(),
        amount: 15_000.0,
    }];
    draft.net_income_per_books = 2_300_000.0;
    draft.non_deductible = vec![Form1702ExRow {
        description: "Nondeductible expense".to_string(),
        amount: 120_001.0,
    }];
    draft.non_taxable_income = vec![Form1702ExRow {
        description: "Interest income".to_string(),
        amount: 20_000.0,
    }];
    draft.overpayment = Form1702ExOverpayment::Refund;
    draft.recompute();
    draft
}

#[test]
fn form_1702ex_sample_payload_is_current() {
    let draft = sample_1702ex();
    assert_eq!(draft.total_amount_payable, -1_000.0);
    let payload = draft
        .to_bir_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 1702EX must validate: {errors:?}"));
    check_sample(
        "1702EX-122025",
        "1702ex-v2018c",
        &payload,
        "739058ef4704ddd829bc7c411ea3eea8a18e4ec4b2e8088614d1799784e2e8f9",
    );
}

/// Every "(more...)" popup in use, driven through the official popups:
/// Item 17I plus two popup rows, Schedule 2 Item 4 plus one, Schedule 3
/// Item 3 plus two (a `maxLength` 12 box: no commas), Items 6 and 8 plus
/// one each. Popup amounts lose their centavos.
fn sample_1702ex_popups() -> Form1702ExDraft {
    let mut draft = sample_1702ex();
    let row = |description: &str, amount: f64| Form1702ExRow {
        description: description.to_string(),
        amount,
    };
    let special = |description: &str, legal_basis: &str, amount: f64| Form1702ExSpecialRow {
        description: description.to_string(),
        legal_basis: legal_basis.to_string(),
        amount,
    };
    draft.net_income_per_books = 996_933.0;
    draft.other_deductions = vec![
        row("Sample expense", 5_000.0),
        row("Rent expense", 6_000.0),
        row("Utilities", 7_000.0),
        row("Supplies", 8_000.0),
        row("Repairs", 9_000.0),
        row("Transport", 3_000.6),
        row("Courier", 2_500.75),
        row("Printing", 1_200.0),
    ];
    draft.special_rows = vec![
        special("Sample deduction", "RA 0000", 15_000.0),
        special("Deduction two", "RA 0002", 2_000.0),
        special("Deduction three", "RA 0003", 3_000.0),
        special("Deduction four", "RA 0004", 4_000.4),
        special("Deduction five", "RA 0005", 5_000.99),
    ];
    draft.non_deductible = vec![
        row("Nondeductible expense", 120_001.0),
        row("Entertainment", 30_000.3),
        row("Penalties", 1_234_567.89),
        row("Donations", 800.0),
    ];
    draft.non_taxable_income = vec![
        row("Row 5", 1_500.0),
        row("Dividends", 6_000.0),
        row("Royalties", 7_000.7),
    ];
    draft.special_deductions_s3 = vec![
        row("Row 7", 1_500.0),
        row("Special one", 8_000.0),
        row("Special two", 9_000.1),
    ];
    draft.recompute();
    draft
}

#[test]
fn form_1702ex_popups_sample_payload_is_current() {
    let draft = sample_1702ex_popups();
    assert_eq!(draft.net_taxable_income, 2_349_301.0);
    assert_eq!(draft.reconciled_taxable_income, 2_349_301.0);
    assert_eq!(draft.popup_len_of("S1I17"), 3);
    let payload = draft
        .to_bir_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 1702EX popups must validate: {errors:?}"));
    let layout = draft.official_layout().expect("extended layout");
    crate::check_sample_with_layout(
        "1702EX-122025-popups",
        &layout,
        &payload,
        "5657b14478055287bc4c100df36a852f123a678c663b71423f2a981f87370745",
    );
}

/// Every sample draft above, for the agent-form probe (`agent_forms`).
pub(crate) fn agent_seeds() -> Vec<serde_json::Value> {
    // Items 48 and 49 (Other Tax Credits/Payments): the draft's two
    // other_credits rows, which the samples above leave empty, so the
    // probe can bind their boxes.
    let mut credits = sample_1702ex();
    credits.other_credits = vec![
        Form1702ExRow {
            description: "Sample credit".to_string(),
            amount: 1_500.0,
        },
        Form1702ExRow {
            description: "Second credit".to_string(),
            amount: 700.0,
        },
    ];
    credits.recompute();
    vec![
        serde_json::to_value(sample_1702ex()).expect("sample serializes"),
        serde_json::to_value(sample_1702ex_popups()).expect("sample serializes"),
        serde_json::to_value(credits).expect("credits sample serializes"),
    ]
}
