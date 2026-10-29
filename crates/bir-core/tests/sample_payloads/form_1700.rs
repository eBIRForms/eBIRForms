//! The steps answer the items in form order (Part V before the Schedule 1
//! employer rows on page 2); profile fields are filled the way `loadBGData`
//! does.

use crate::sample_payloads::penalty_items::{self, Deadline, day};
// The samples enter their penalties by hand (`keep_entered_penalties`).
use crate::{check_sample, dummy_profile};
use bir_core::forms::form_1700::{
    Form1700CivilStatus, Form1700Column, Form1700Draft, Form1700Employer, Form1700FilingStatus,
    Form1700Spouse, Form1700TaxpayerType,
};
use bir_core::forms::queueable::QueueableForm as _;

fn sample_1700() -> Form1700Draft {
    let mut draft = Form1700Draft::new_from_profile(&dummy_profile("1700", "Individual"), 2025);
    draft.taxpayer_name = "Dummy, Sample Taxpayer".to_string();
    draft.taxpayer_type = Form1700TaxpayerType::Employee;
    draft.birth_date = "01/15/1980".to_string();
    draft.citizenship = "Filipino".to_string();
    draft.foreign_tax_credits = Some(false);
    draft.civil_status = Form1700CivilStatus::Single;
    draft.taxpayer = Form1700Column {
        non_taxable: 90_000.0,
        other_income: 10_000.49,
        other_credits: 100.0,
        surcharge: 1_000.0,
        ..Form1700Column::default()
    };
    draft.other_income_description = "Prize".to_string();
    draft.other_credits_description = "Sample credit".to_string();
    draft.employers = vec![Form1700Employer {
        for_spouse: false,
        name: "Sample Employer Inc".to_string(),
        name2: String::new(),
        tin: "12345678800000".to_string(),
        regular: 600_000.5,
        flat: 0.0,
        withheld: 40_000.0,
    }];
    draft.keep_entered_penalties();
    draft.recompute();
    draft
}

#[test]
fn form_1700_sample_payload_is_current() {
    let draft = sample_1700();
    assert_eq!(draft.aggregate_amount_payable, 7_400.0);
    let payload = draft
        .to_bir_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 1700 must validate: {errors:?}"));
    check_sample(
        "1700-2025",
        "1700-v2013",
        &payload,
        "d2862acdc28a0430e1b02f8a0d2536bb1c38c0ef32baa34561f399570006bb3f",
    );
}

/// Joint filing: the spouse is an employee with their own employer row.
fn sample_1700_joint() -> Form1700Draft {
    let mut draft = sample_1700();
    draft.civil_status = Form1700CivilStatus::Married;
    draft.spouse_has_income = Some(true);
    draft.filing_status = Form1700FilingStatus::Joint;
    draft.spouse = Form1700Spouse {
        tin: "12345678800000".to_string(),
        rdo_code: "039".to_string(),
        taxpayer_type: Form1700TaxpayerType::Employee,
        name: "Dummy, Sample Spouse".to_string(),
        contact_number: "09170000001".to_string(),
        citizenship: "Filipino".to_string(),
        foreign_tax_credits: Some(false),
        foreign_tax_number: String::new(),
    };
    draft.employers.push(Form1700Employer {
        for_spouse: true,
        name: "Sample Spouse Employer".to_string(),
        name2: String::new(),
        tin: "12345678800000".to_string(),
        regular: 400_000.0,
        flat: 0.0,
        withheld: 20_000.0,
    });
    draft.keep_entered_penalties();
    draft.recompute();
    draft
}

#[test]
fn form_1700_joint_sample_payload_is_current() {
    let draft = sample_1700_joint();
    assert_eq!(draft.aggregate_amount_payable, 9_900.0);
    let payload = draft
        .to_bir_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy joint 1700 must validate: {errors:?}"));
    check_sample(
        "1700-2025-joint",
        "1700-v2013",
        &payload,
        "6500d6fc3bf03e09f26b54091e6497b6dad663b62602f496807479fc852e07f4",
    );
}

/// Six employers: rows 1 and 4 on the page, then rows `4_2` and `4_3` through
/// the "(add more...)" popup (Save, then Close). Item 4 reads OTHERS with the
/// popup subtotal; the popup's rows are written where `addRow_schedule1`
/// leaves them in `frmMain`. The steps first give the popup's empty tables
/// the `<tbody>` IE inserts implicitly (jsdom does not), which the page's
/// `$("#tbl… tbody").append(...)` needs.
fn sample_1700_others() -> Form1700Draft {
    let mut draft = sample_1700();
    draft.employers.resize_with(3, Form1700Employer::default);
    for (name, tin, regular, withheld) in [
        (
            "Fourth Employer Corp",
            "12345678800000",
            100_000.25,
            5_000.0,
        ),
        ("Fifth Employer Corp", "12345678800000", 50_000.4, 2_500.1),
        ("Sixth Employer", "12345678800001", 25_000.0, 1_000.0),
    ] {
        draft.employers.push(Form1700Employer {
            for_spouse: false,
            name: name.to_string(),
            name2: String::new(),
            tin: tin.to_string(),
            regular,
            flat: 0.0,
            withheld,
        });
    }
    draft.keep_entered_penalties();
    draft.recompute();
    draft
}

#[test]
fn form_1700_sample_with_popup_rows_is_current() {
    let draft = sample_1700_others();
    assert!(draft.schedule_is_folded());
    let payload = draft
        .to_bir_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 1700 with popup rows must validate: {errors:?}"));
    let layout = draft.official_layout().expect("extended layout");
    crate::check_sample_with_layout(
        "1700-2025-others",
        &layout,
        &payload,
        "bf9ca8160cedfdb509fbc16145f5b755da2a299f0232ca4c7644939b0c00c93f",
    );
}

/// Calendar year 2025: due 15 April 2026.
#[test]
fn form_1700_penalty_items_follow_the_shared_model() {
    penalty_items::check(sample_1700, Deadline::Calendar(day(2026, 4, 15)));
}

/// Every sample draft above, for the agent-form probe (`agent_forms`).
pub(crate) fn agent_seeds() -> Vec<serde_json::Value> {
    vec![
        serde_json::to_value(sample_1700()).expect("sample serializes"),
        // The spouse's items and column B are written only on a joint return.
        serde_json::to_value(sample_1700_joint()).expect("sample serializes"),
        // Items 25, 55 and 56 are written only on an amended return whose
        // filers claim foreign tax credits. The "(add more...)" popup rows
        // are left out: folding moves Item 4's employer to row 4_1, and the
        // screen writes those rows itself (bir-shell rows_1700).
        serde_json::to_value(sample_1700_joint_amended_credits()).expect("sample serializes"),
    ]
}

fn sample_1700_joint_amended_credits() -> Form1700Draft {
    let mut draft = sample_1700_joint();
    draft.is_amended = true;
    draft.foreign_tax_credits = Some(true);
    draft.foreign_tax_number = "FTN-0001".to_string();
    draft.spouse.foreign_tax_credits = Some(true);
    draft.spouse.foreign_tax_number = "FTN-0002".to_string();
    draft.taxpayer.previously_filed = 100.0;
    draft.taxpayer.foreign_tax_credits = 50.0;
    draft.spouse_column.previously_filed = 200.0;
    draft.spouse_column.foreign_tax_credits = 75.0;
    draft.recompute();
    draft
}
