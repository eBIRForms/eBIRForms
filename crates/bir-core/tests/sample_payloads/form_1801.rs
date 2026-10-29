use crate::sample_payloads::penalty_items::{self, Deadline, day};
use crate::{check_sample, dummy_profile};
use bir_core::forms::form_1801::{
    Form1801Business, Form1801Draft, Form1801Frequency, Form1801Particular, Form1801RealProperty,
    Form1801Shares, Form1801Split,
};
use bir_core::forms::queueable::QueueableForm as _;

fn split(exclusive: f64, conjugal: f64) -> Form1801Split {
    Form1801Split {
        exclusive,
        conjugal,
    }
}

fn sample_1801() -> Form1801Draft {
    let mut d = Form1801Draft::new_from_profile(&dummy_profile("1801", "Estate"), 2025, 1);
    d.death_month = 6;
    d.death_day = 5;
    d.death_year = 2025;
    d.number_of_attached_sheets = 1;
    d.non_resident_alien = Some(false);
    d.administrator_name = "Maria Sample-Executor".into();
    d.administrator_tin = "111-222-333".into();
    d.administrator_branch_code = "00000".into();
    d.extension_to_file = true;
    d.set_installment_granted(true);
    d.installment_frequency = Some(Form1801Frequency::Quarterly);
    d.real_properties = vec![Form1801RealProperty {
        title_number: "T-12345".into(),
        tax_declaration_number: "TD-001".into(),
        location: "Quezon City".into(),
        lot_or_improvement: "Lot".into(),
        area: "200".into(),
        classification: "RR".into(),
        fmv_per_tax_declaration: "1000000".into(),
        fmv_per_zonal_value: "1200000".into(),
        value: split(1_200_000.005, 0.0),
    }];
    d.family_homes = vec![Form1801RealProperty {
        title_number: "T-67890".into(),
        tax_declaration_number: "TD-002".into(),
        location: "Pasig City".into(),
        area: "150".into(),
        classification: "RC".into(),
        fmv_per_tax_declaration: "1800000".into(),
        fmv_per_zonal_value: "2000000".into(),
        value: split(0.0, 2_000_000.0),
        ..Default::default()
    }];
    d.shares = vec![Form1801Shares {
        corporation: "Sample Corp".into(),
        listing: "Not Listed".into(),
        certificate_number: "C-100".into(),
        number_of_shares: "1000".into(),
        value_per_share: "300.00".into(),
        value: split(300_000.55, 0.0),
    }];
    d.other_personal = vec![
        Form1801Particular {
            particulars: "Car".into(),
            value: split(50_000.0, 0.0),
        },
        Form1801Particular {
            particulars: "Jewelry".into(),
            value: split(0.0, 25_000.125),
        },
    ];
    d.taxable_transfers = vec![Form1801Particular {
        particulars: "Cash in bank, Sample Bank 0001".into(),
        value: split(10_000.0, 0.0),
    }];
    d.business_interests = vec![Form1801Business {
        name: "Sample Store".into(),
        address: "Makati City".into(),
        rdo_code: "040".into(),
        value: split(40_000.0, 0.0),
    }];
    d.ordinary_deductions.claims_against_estate = split(100_000.0, 0.0);
    d.ordinary_deductions.settlement_losses = split(0.0, 5_000.0);
    d.standard_deduction = 5_000_000.0;
    d.family_home_deduction = 1_000_000.0;
    d.installment_year = "2026".into();
    d.surcharge = 25.5;
    d.keep_entered_penalties();
    d.recompute();
    d
}

#[test]
fn form_1801_sample_payload_is_current() {
    let draft = sample_1801();
    let payload = draft
        .to_bir_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 1801 must validate: {errors:?}"));
    check_sample(
        "1801-06052025",
        "1801-v2018",
        &payload,
        "4e9ee1da726331f2be70c9e5734821fe0ef622f0b2369ff3c84c5adbddb780aa",
    );
}

/// A return with tax due and rows added with "Add row": its layout is the
/// generated one extended by `Form1801Draft::official_layout`, so the shared
/// `check_sample` (fixed layout) is replaced by the same checks against it.
fn sample_1801_with_tax_and_added_rows() -> Form1801Draft {
    let mut d = sample_1801();
    d.death_month = 9;
    d.death_day = 15;
    d.death_year = 2025;
    d.real_properties.push(Form1801RealProperty {
        title_number: "T-22222".into(),
        tax_declaration_number: "TD-003".into(),
        location: "Cebu City".into(),
        lot_or_improvement: "Improvement".into(),
        area: "80".into(),
        classification: "CR".into(),
        fmv_per_tax_declaration: "2500000".into(),
        fmv_per_zonal_value: "3000000".into(),
        value: split(3_000_000.0, 0.0),
    });
    d.real_properties.push(Form1801RealProperty {
        title_number: "T-33333".into(),
        tax_declaration_number: "TD-004".into(),
        location: "Davao City".into(),
        lot_or_improvement: "Lot".into(),
        area: "500".into(),
        classification: "A".into(),
        fmv_per_tax_declaration: "1500000".into(),
        fmv_per_zonal_value: "1750000.50".into(),
        value: split(0.0, 1_750_000.505),
    });
    d.other_personal.push(Form1801Particular {
        particulars: "Paintings".into(),
        value: split(120_000.0, 0.0),
    });
    d.business_interests.push(Form1801Business {
        name: "Second Sample Shop".into(),
        address: "Taguig City".into(),
        rdo_code: "044".into(),
        value: split(0.0, 60_000.0),
    });
    d.surcharge = 1_000.0;
    d.interest = 250.25;
    d.keep_entered_penalties();
    d.recompute();
    d
}

#[test]
fn form_1801_sample_with_tax_due_and_added_rows_is_current() {
    let draft = sample_1801_with_tax_and_added_rows();
    assert!(draft.estate_tax_due() > 0.0);
    let payload = draft
        .to_bir_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 1801 must validate: {errors:?}"));
    let layout = draft.official_layout().expect("extended layout");
    crate::check_sample_with_layout(
        "1801-09152025",
        &layout,
        &payload,
        "e053260aa0353fb3a9c35d87a78f719fdc520763947a2f054b02ce917359c6b3",
    );
}

/// Every Part V table past the six rows the page used to show here, and
/// Schedule 1 past the old 20-row cap: `addRow_*` has no limit.
fn sample_1801_many_rows() -> Form1801Draft {
    let mut d = sample_1801();
    d.death_month = 9;
    d.death_day = 16;
    d.death_year = 2025;
    let classes = ["RR", "CR", "A", "RC", "CC", "I", "APD"];
    for k in 2..=21usize {
        d.real_properties.push(Form1801RealProperty {
            title_number: format!("T-{k:05}"),
            tax_declaration_number: format!("TD-1{k:02}"),
            location: format!("Lot {k}, Cebu City"),
            lot_or_improvement: if k % 2 == 0 { "Lot" } else { "Improvement" }.into(),
            area: format!("{}", 50 + k),
            classification: classes[k % classes.len()].into(),
            fmv_per_tax_declaration: format!("{}", k * 90_000),
            fmv_per_zonal_value: format!("{}", k * 100_000),
            value: split(k as f64 * 100_000.125, 0.0),
        });
    }
    for k in 2..=7usize {
        d.family_homes.push(Form1801RealProperty {
            title_number: format!("F-{k:03}"),
            tax_declaration_number: format!("FD-{k:03}"),
            location: format!("Home {k}, Pasig City"),
            area: format!("{}", 100 + k),
            classification: classes[(k + 1) % classes.len()].into(),
            fmv_per_tax_declaration: format!("{}", k * 50_000),
            fmv_per_zonal_value: format!("{}", k * 60_000),
            value: split(0.0, k as f64 * 1_000.5),
            ..Default::default()
        });
        d.shares.push(Form1801Shares {
            corporation: format!("Corp {k}"),
            listing: if k % 2 == 0 { "Listed" } else { "Not Listed" }.into(),
            certificate_number: format!("C-{k}00"),
            number_of_shares: format!("{}", k * 10),
            value_per_share: "10.00".into(),
            value: split(k as f64 * 100.0, 0.0),
        });
        d.business_interests.push(Form1801Business {
            name: format!("Shop {k}"),
            address: format!("Street {k}, Taguig City"),
            rdo_code: "044".into(),
            value: split(0.0, k as f64 * 500.0),
        });
        d.taxable_transfers.push(Form1801Particular {
            particulars: format!("Transfer {k}"),
            value: split(k as f64 * 250.0, 0.0),
        });
    }
    for k in 3..=7usize {
        d.other_personal.push(Form1801Particular {
            particulars: format!("Item {k}"),
            value: split(k as f64 * 75.25, 0.0),
        });
    }
    d.recompute();
    d
}

#[test]
fn form_1801_sample_with_many_rows_is_current() {
    let draft = sample_1801_many_rows();
    assert_eq!(draft.real_properties.len(), 21);
    for n in [
        draft.family_homes.len(),
        draft.shares.len(),
        draft.other_personal.len(),
        draft.taxable_transfers.len(),
        draft.business_interests.len(),
    ] {
        assert_eq!(n, 7);
    }
    let payload = draft
        .to_bir_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 1801 must validate: {errors:?}"));
    let layout = draft.official_layout().expect("extended layout");
    crate::check_sample_with_layout(
        "1801-09162025",
        &layout,
        &payload,
        "88a21bbbba7be3539d3adda4bb49d632bcf3700dd22124d853b17e4ef1085ded",
    );
}

/// Due one year after the death (maintainer decision (1800/1801 penalties run
/// from the due date)): the penalties run from
/// that day; the calendar has no 1801 deadline.
#[test]
fn form_1801_with_tax_and_added_rows_penalty_items_follow_the_shared_model() {
    penalty_items::check(
        sample_1801_with_tax_and_added_rows,
        // Item 1 is 09/15/2025: due Tuesday, September 15, 2026.
        Deadline::Typed {
            due: day(2026, 9, 15),
        },
    );
}

/// Every sample draft above, for the agent-form probe (`agent_forms`).
pub(crate) fn agent_seeds() -> Vec<serde_json::Value> {
    vec![
        serde_json::to_value(sample_1801()).expect("sample serializes"),
        serde_json::to_value(sample_1801_with_tax_and_added_rows()).expect("sample serializes"),
        serde_json::to_value(sample_1801_many_rows()).expect("sample serializes"),
    ]
}
