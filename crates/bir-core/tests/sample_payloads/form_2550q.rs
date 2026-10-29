use crate::dummy_profile;
use bir_core::forms::form_2550q::{
    Form2550QAdditionalItemRow, Form2550QAdvanceVatRow, Form2550QCapitalGoodRow,
    Form2550QCreditableVatRow, Form2550QDate, Form2550QDraft, Form2550QTaxpayerClassification,
};
use bir_core::forms::queueable::QueueableForm;

fn sample_2550q() -> Form2550QDraft {
    let mut draft =
        Form2550QDraft::new_from_profile(&dummy_profile("2550Q", "Corporation"), 2025, 1);
    draft.taxpayer_classification = Some(Form2550QTaxpayerClassification::Small);
    draft.part_iv.item_31a_vatable_sales = Some(1_250_000.0);
    draft.part_iv.item_32a_zero_rated_sales = Some(200_000.0);
    draft.part_iv.item_33a_exempt_sales = Some(50_000.0);
    draft.part_iv.item_44a_domestic_purchases = Some(400_000.0);
    // `saveEncryptedProfile` stamps the PC date; the steps pin it to this day.
    draft.date_filed = Form2550QDate::new(2025, 4, 25).ok();
    draft.recompute();
    draft
}

/// [`crate::check_sample`] for 2550Q, whose upload appends
/// `<dateFiled>yyyy/mm/dd</dateFiled>\n` after the layout's trailer.
fn check_sample_2550q(
    draft: &Form2550QDraft,
    stem: &str,
    plaintext: &str,
    _official_encrypt_sha256: &str,
) {
    let (controls, date_filed) = plaintext
        .split_once("<dateFiled>")
        .expect("2550Q upload appends <dateFiled>");
    let date = date_filed
        .strip_suffix("</dateFiled>\n")
        .expect("<dateFiled> closes the upload");
    assert_eq!(date.len(), 10, "dateFiled is yyyy/mm/dd");
    let layout = draft.official_layout().expect("official layout");
    let fields = bir_core::official_xml::read(&layout, controls)
        .expect("sample follows the official layout");
    assert!(!fields.is_empty());

    let dir = crate::samples_dir();
    let plain_path = dir.join(format!("{stem}.plain.xml"));
    let updating = std::env::var_os("UPDATE_SAMPLE_PAYLOADS").is_some();
    if updating {
        std::fs::write(&plain_path, plaintext).unwrap();
    }
    let official_path = dir.join(format!("{stem}.official.xml"));
    let official = std::fs::read_to_string(&official_path)
        .unwrap_or_else(|e| panic!("missing {}: {e}", official_path.display()));
    assert_eq!(
        plaintext, official,
        "{stem}: our plaintext differs from the official upload (saveEncryptedProfile) output"
    );
    if updating {
        return;
    }
    assert_eq!(std::fs::read_to_string(&plain_path).unwrap(), plaintext);
}

#[test]
fn form_2550q_sample_payload_is_current() {
    let draft = sample_2550q();
    // The exact plaintext the generic queue worker uploads.
    let payload = draft
        .official_payload()
        .unwrap_or_else(|errors| panic!("dummy 2550Q must validate: {errors:?}"));
    check_sample_2550q(
        &draft,
        "2550Q-122025Q1",
        &payload,
        "bb3a8b4412d8dc50ae619bd3249f0d47de01418a4a46a358239fb51ee9dd8d3f",
    );
}

fn date(year: u16, month: u8, day: u8) -> Option<Form2550QDate> {
    Some(Form2550QDate::new(year, month, day).expect("valid date"))
}

/// The 2550Q sample with three rows in each of Schedules 1, 3 and 4, entered
/// through the official `addlistSched1/3/4` modals.
fn sample_2550q_schedules() -> Form2550QDraft {
    let mut draft = sample_2550q();
    let capital = |d, code: &str, what: &str, amount, tax, est, rec| Form2550QCapitalGoodRow {
        purchase_or_import_date: d,
        source_code: code.to_string(),
        description: what.to_string(),
        purchase_or_import_amount: Some(amount),
        input_tax: Some(tax),
        estimated_life_months: Some(est),
        recognized_life_months: Some(rec),
        ..Form2550QCapitalGoodRow::default()
    };
    draft.schedule_1 = vec![
        capital(
            date(2021, 1, 15),
            "D",
            "OFFICE BUILDING",
            1_000_000.0,
            120_000.0,
            60,
            60,
        ),
        capital(
            date(2020, 6, 30),
            "I",
            "FORKLIFT",
            500_000.0,
            60_000.0,
            48,
            48,
        ),
        capital(
            date(2019, 11, 2),
            "D",
            "GENERATOR SET",
            250_000.5,
            30_000.06,
            36,
            40,
        ),
    ];
    let creditable = |from, to, agent: &str, income, tax| Form2550QCreditableVatRow {
        period_from: from,
        period_to: to,
        withholding_agent_name: agent.to_string(),
        income_payment: Some(income),
        tax_withheld: Some(tax),
        ..Form2550QCreditableVatRow::default()
    };
    draft.schedule_3 = vec![
        creditable(
            date(2025, 1, 1),
            date(2025, 1, 31),
            "ABC GOVERNMENT AGENCY",
            100_000.0,
            5_000.0,
        ),
        creditable(
            date(2025, 2, 1),
            date(2025, 2, 28),
            "XYZ STATE COLLEGE",
            200_000.0,
            10_000.0,
        ),
        creditable(
            date(2025, 3, 1),
            date(2025, 3, 31),
            "DEPARTMENT OF EDUCATION",
            50_000.25,
            2_500.01,
        ),
    ];
    let advance = |from, to, miller: &str, receipt: &str, amount| Form2550QAdvanceVatRow {
        period_from: from,
        period_to: to,
        miller_name: miller.to_string(),
        taxpayer_name: "SAMPLE DUMMY TAXPAYER".to_string(),
        official_receipt_number: receipt.to_string(),
        amount_paid: Some(amount),
        ..Form2550QAdvanceVatRow::default()
    };
    draft.schedule_4 = vec![
        advance(
            date(2025, 1, 5),
            date(2025, 1, 20),
            "PRIME RICE MILL",
            "OR12345",
            15_000.0,
        ),
        advance(
            date(2025, 2, 3),
            date(2025, 2, 18),
            "GOLDEN GRAIN MILL",
            "OR22222",
            8_500.5,
        ),
        advance(
            date(2025, 3, 10),
            date(2025, 3, 25),
            "NORTH SUGAR MILL",
            "A1B2C3",
            1_200.75,
        ),
    ];
    draft
        .ensure_repeating_row_ids()
        .expect("rows get stable identities");
    draft.recompute();
    draft
}

#[test]
fn form_2550q_schedule_rows_sample_payload_is_current() {
    let draft = sample_2550q_schedules();
    let payload = draft
        .official_payload()
        .unwrap_or_else(|errors| panic!("2550Q with schedule rows must validate: {errors:?}"));
    check_sample_2550q(
        &draft,
        "2550Q-122025Q1-schedules",
        &payload,
        "6c689c3c28fe65e8ff0e973203ff62370a38cd2be8f1bda212f6d471883f0a14",
    );
}

/// The 2550Q sample with rows in the item 19, 42, 47 and 56 "(Add more...)"
/// popups, entered through the official `addTblSpecifyNo*` / `addColNo*` /
/// `saveAndCloseNo*` functions.
fn sample_2550q_specify() -> Form2550QDraft {
    let mut draft = sample_2550q();
    let rows = |rows: &[(&str, f64)]| {
        rows.iter()
            .map(|(description, amount)| Form2550QAdditionalItemRow {
                instance_id: None,
                description: (*description).to_string(),
                amount: Some(*amount),
            })
            .collect::<Vec<_>>()
    };
    draft.item_19_additional_rows = rows(&[
        ("REFUND APPLIED", 100.0),
        ("TAX CREDIT CERTIFICATE", 2_200.5),
        ("OTHER PAYMENT", 3.25),
    ]);
    draft.item_42_additional_rows = rows(&[("INPUT TAX A", 1_000.0), ("INPUT TAX B", 234.56)]);
    draft.item_47_additional_rows = rows(&[
        ("PURCHASE A", 5_000.0),
        ("PURCHASE B", 1_234.5),
        ("PURCHASE C", 10.05),
    ]);
    draft.item_56_additional_rows = rows(&[
        ("DEDUCTION A", 300.0),
        ("DEDUCTION B", 45.67),
        ("DEDUCTION C", 1_000.01),
    ]);
    draft
        .ensure_repeating_row_ids()
        .expect("rows get stable identities");
    draft.recompute();
    draft
}

#[test]
fn form_2550q_specify_popup_rows_sample_payload_is_current() {
    let draft = sample_2550q_specify();
    let payload = draft
        .official_payload()
        .unwrap_or_else(|errors| panic!("2550Q with popup rows must validate: {errors:?}"));
    check_sample_2550q(
        &draft,
        "2550Q-122025Q1-specify",
        &payload,
        "a3a4649262a958bbff252db21cbe92e6abe51f24e7521f3a204ef83bbe071498",
    );
}

/// Every sample draft above, for the agent-form probe (`agent_forms`).
pub(crate) fn agent_seeds() -> Vec<serde_json::Value> {
    vec![
        serde_json::to_value(sample_2550q()).expect("sample serializes"),
        serde_json::to_value(sample_2550q_schedules()).expect("sample serializes"),
        serde_json::to_value(sample_2550q_specify()).expect("sample serializes"),
    ]
}
