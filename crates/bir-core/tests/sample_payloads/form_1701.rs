use crate::{dummy_profile, samples_dir};
use bir_core::forms::form_1701::{
    Form1701AmountSection as S, Form1701Atc, Form1701CivilStatus, Form1701DeductionMethod,
    Form1701Draft, Form1701EmployerRow, Form1701JointFilingStatus, Form1701Party,
    Form1701SpecialDeductionRow, Form1701SpouseType, Form1701TaxpayerType,
};
use bir_core::forms::queueable::QueueableForm as _;

const TP: Form1701Party = Form1701Party::Taxpayer;
const SP: Form1701Party = Form1701Party::Spouse;

/// Mixed income (II013), graduated rates with OSD, rounding cases.
fn sample_1701() -> Form1701Draft {
    let mut d =
        Form1701Draft::new_from_profile(&dummy_profile("1701v2018", "Individual"), 2025, 12);
    d.taxpayer_type = Some(Form1701TaxpayerType::SingleProprietor);
    d.taxpayer_also_compensation_earner = true;
    d.atc = Some(Form1701Atc::Ii013);
    d.date_of_birth = "01/15/1980".into();
    d.citizenship = "Filipino".into();
    d.claims_foreign_tax_credits = Some(false);
    d.civil_status = Some(Form1701CivilStatus::Single);
    d.has_exempt_income = Some(false);
    d.has_special_rate_income = Some(false);
    d.deduction_method = Some(Form1701DeductionMethod::Osd);
    d.employers[0] = Form1701EmployerRow {
        owner: Some(TP),
        employer_name: "Sample Employer, Inc.".into(),
        employer_tin: "123-456-788-00000".into(),
        compensation_income: Some(480_000.005),
        tax_withheld: Some(25_000.0),
    };
    d.set_amount(S::Schedule2, 5, TP, Some(90_000.0));
    d.set_amount(S::Schedule3, 8, TP, Some(1_250_000.5));
    d.set_amount(S::Schedule3, 9, TP, Some(300_000.25));
    d.computations
        .schedule_3_descriptions
        .insert(19, "Rental income".into());
    d.set_amount(S::Schedule3, 19, TP, Some(12_345.67));
    d.set_amount(S::PartVii, 1, TP, Some(10_000.0));
    d.set_amount(S::PartVii, 2, TP, Some(20_000.5));
    d.set_amount(S::PartIi, 27, TP, Some(500.0));
    d.set_amount(S::PartIi, 28, TP, Some(250.25));
    // Entered by hand (Items 27-29 are on the shared penalty model since PR
    // 7): they file as typed, whatever the engine computes.
    d.keep_entered_penalties();
    d.recompute();
    d
}

/// Joint filing: a professional on the 8% rate and a spouse with mixed
/// income on graduated rates with itemized deductions and NOLCO.
fn sample_1701_joint() -> Form1701Draft {
    let mut d =
        Form1701Draft::new_from_profile(&dummy_profile("1701v2018", "Individual"), 2025, 12);
    d.taxpayer_type = Some(Form1701TaxpayerType::Professional);
    d.atc = Some(Form1701Atc::Ii017);
    d.date_of_birth = "03/01/1975".into();
    d.citizenship = "Filipino".into();
    d.claims_foreign_tax_credits = Some(false);
    d.civil_status = Some(Form1701CivilStatus::Married);
    d.spouse_has_income = Some(true);
    d.joint_filing_status = Some(Form1701JointFilingStatus::Joint);
    d.has_exempt_income = Some(false);
    d.has_special_rate_income = Some(false);
    d.spouse.tin = "123-456-788-00001".into();
    d.spouse_rdo_code = "039".into();
    d.spouse.filer_type = Some(Form1701SpouseType::SingleProprietor);
    d.spouse_also_compensation_earner = true;
    d.spouse.atc = Some(Form1701Atc::Ii013);
    d.spouse.name = "Sample Dummy Spouse".into();
    d.spouse.contact_number = "09170000001".into();
    d.spouse.citizenship = "Filipino".into();
    d.spouse.claims_foreign_tax_credits = Some(false);
    d.spouse.has_exempt_income = Some(false);
    d.spouse.has_special_rate_income = Some(false);
    d.spouse.deduction_method = Some(Form1701DeductionMethod::Itemized);
    d.employers[1] = Form1701EmployerRow {
        owner: Some(SP),
        employer_name: "Spouse Employer Corp".into(),
        employer_tin: "123-456-788-00002".into(),
        compensation_income: Some(300_000.0),
        tax_withheld: Some(5_000.0),
    };
    d.set_amount(S::Schedule3, 26, TP, Some(1_800_000.4));
    d.computations
        .schedule_3_descriptions
        .insert(27, "Interest income".into());
    d.set_amount(S::Schedule3, 27, TP, Some(100_000.0));
    d.set_amount(S::Schedule3, 29, TP, Some(250_000.0));
    d.set_amount(S::PartIx, 1, TP, Some(1_650_000.0));
    d.set_amount(S::PartVii, 3, TP, Some(1_000.0));
    d.set_amount(S::Schedule3, 8, SP, Some(800_000.0));
    d.set_amount(S::Schedule3, 9, SP, Some(200_000.5));
    d.set_amount(S::Schedule3, 11, SP, Some(10_000.0));
    d.set_amount(S::Schedule4, 1, SP, Some(50_000.0));
    d.set_amount(S::Schedule4, 2, SP, Some(20_000.75));
    d.computations.schedule_4_item_17[3].spouse = Some(1_000.0);
    d.computations.schedule_4_item_17d_description = "Miscellaneous".into();
    d.computations.schedule_5_spouse[0] = Form1701SpecialDeductionRow {
        description: "Special deduction".into(),
        legal_basis: "RA 1234".into(),
        amount: Some(5_000.0),
    };
    let row = &mut d.computations.schedule_6_spouse_nolco[0];
    row.year_incurred = "2023".into();
    row.amount = Some(30_000.0);
    row.applied_previous_years = Some(10_000.0);
    row.applied_current_year = Some(15_000.0);
    d.set_amount(S::PartIx, 1, SP, Some(498_999.0));
    // No penalties entered: both columns file the zeros they hold (Items
    // 27-29 are on the shared penalty model since PR 7).
    d.keep_entered_penalties();
    d.recompute();
    d
}

#[test]
fn form_1701_joint_sample_matches_the_official_page() {
    let payload = sample_1701_joint()
        .to_official_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy joint 1701 must validate: {errors:?}"));
    let dir = samples_dir();
    let stem = "1701-122025-joint";
    if std::env::var_os("UPDATE_SAMPLE_PAYLOADS").is_some() {
        std::fs::write(dir.join(format!("{stem}.plain.xml")), &payload).unwrap();
    }
    let official = std::fs::read_to_string(dir.join(format!("{stem}.official.xml"))).unwrap();
    assert_eq!(
        payload, official,
        "{stem}: differs from the official saveEncryptedProfile() output"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join(format!("{stem}.plain.xml"))).unwrap(),
        payload
    );
}

/// Our plaintext against the official page. The 1701 layout names the RDO
/// selects differently from the page (see `form_1701_official`), so this
/// sample is compared directly instead of through `check_sample`.
#[test]
fn form_1701_sample_payload_matches_the_official_page() {
    let payload = sample_1701()
        .to_official_xml_payload()
        .unwrap_or_else(|errors| panic!("dummy 1701 must validate: {errors:?}"));
    let dir = samples_dir();
    let stem = "1701-122025";
    if std::env::var_os("UPDATE_SAMPLE_PAYLOADS").is_some() {
        std::fs::write(dir.join(format!("{stem}.plain.xml")), &payload).unwrap();
    }
    let official = std::fs::read_to_string(dir.join(format!("{stem}.official.xml"))).unwrap();
    assert_eq!(
        payload, official,
        "{stem}: differs from the official saveEncryptedProfile() output"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join(format!("{stem}.plain.xml"))).unwrap(),
        payload
    );
}

/// The joint sample amended, with the entries the other samples leave
/// blank: both foreign tax numbers (item 14 and Part IV item 9, kept only
/// while items 13 and 8 are Yes), Schedule 3 item 20's description, Part VII item 6 (tax
/// paid in the return previously filed, kept only on an amended return) and
/// Part IX's particulars. For the agent-form probe only (PR 7): without it
/// the probe saw those boxes blank on every sample and could not bind them.
fn sample_1701_joint_details() -> Form1701Draft {
    let mut d = sample_1701_joint();
    d.is_amended = true;
    d.claims_foreign_tax_credits = Some(true);
    d.foreign_tax_number = "TP-FTN-0001".into();
    d.spouse.claims_foreign_tax_credits = Some(true);
    d.spouse.foreign_tax_number = "SP-FTN-0001".into();
    d.computations
        .schedule_3_descriptions
        .insert(20, "Royalty income".into());
    d.set_amount(S::Schedule3, 20, SP, Some(2_000.0));
    d.set_amount(S::PartVii, 6, TP, Some(1_500.0));
    d.set_amount(S::PartVii, 6, SP, Some(700.0));
    for (item, particulars) in [
        (2, "Non-deductible expense"),
        (3, "Taxable other income"),
        (4, "Other addition"),
        (6, "Non-taxable income"),
        (7, "Income subjected to final tax"),
        (8, "Special deduction"),
        (9, "Other allowable deduction"),
    ] {
        d.computations
            .part_ix_descriptions
            .insert(item, particulars.into());
        d.set_amount(S::PartIx, item, TP, Some(100.0 * f64::from(item)));
    }
    d.recompute();
    d
}

/// Every sample draft above, for the agent-form probe (`agent_forms`).
pub(crate) fn agent_seeds() -> Vec<serde_json::Value> {
    vec![
        serde_json::to_value(sample_1701()).expect("sample serializes"),
        serde_json::to_value(sample_1701_joint()).expect("sample serializes"),
        serde_json::to_value(sample_1701_joint_details()).expect("sample serializes"),
    ]
}

/// Items 27-29 of both columns on the shared penalty model (PR 7, R8.0),
/// checked the way `penalty_items::check` checks every wired form. That check
/// reads each item by a draft field named after it (`surcharge`); 1701 keeps
/// its amounts by item number (`computations.part_ii.28.taxpayer`), and a
/// draft stored while the items were typed boxes keeps all three amounts, a
/// zero too, so it runs here with those two differences, as 1701Q's does.
/// Taxable year 2025 is due 15 April 2026 (calendar_rules.rs "April 15 /
/// 15th day of 4th month following taxable year"); the sample is filed late
/// on 13 October 2026. Rates: surcharge 25%, interest 12% a year, compromise
/// from the RMO 7-2015 schedule; centavos, as every 1701 amount.
#[test]
fn form_1701_penalty_items_follow_the_shared_model() {
    use crate::sample_payloads::penalty_items::day;
    use bir_core::forms::FilingStatus;
    use bir_core::forms::penalty_items::with_clock;
    use bir_core::forms::queueable::QueueableForm;
    use bir_core::penalties::compromise::lookup_compromise;
    use bir_core::penalties::{PenaltyComponent, PenaltyProfile};

    let filed = day(2026, 10, 13);
    let compute = |draft: &mut Form1701Draft| with_clock(filed, || draft.recompute());
    let declarations = Form1701Draft::penalty_items();
    assert_eq!(declarations.len(), 2);
    let money = |text: &str| text.replace(',', "").parse::<f64>().unwrap();
    let cents = |value: f64| (value * 100.0).round() / 100.0;

    // The joint sample's taxpayer and spouse both owe tax: both compute.
    let mut late = sample_1701_joint();
    late.penalty.penalty_overrides = Default::default();
    late.spouse_penalty.penalty_overrides = Default::default();
    compute(&mut late);
    for (items, column) in declarations.iter().zip(["A", "B"]) {
        assert!((items.applies)(&late), "column {column} applies");
        let computed = (items.state)(&late).computed_penalties.clone().unwrap();
        assert_eq!(
            computed.inputs.due_date,
            day(2026, 4, 15),
            "column {column}"
        );
        assert_eq!(computed.inputs.filing_date, filed);
        let basic = money(&late.field_map()[&format!("frm1701:txtPg1I26{column}")]);
        assert!(basic > 0.0, "column {column} has tax still due");
        assert_eq!(computed.inputs.basic_tax_due, basic);
        let days = computed.inputs.days_late as f64;
        assert_eq!(days, (filed - day(2026, 4, 15)).num_days() as f64);
        let expected = [
            cents(basic * 0.25),
            cents(basic * 0.12 * days / 365.0),
            cents(lookup_compromise(
                basic,
                0.0,
                PenaltyProfile::StandardFiling,
            )),
        ];
        let fields = late.field_map();
        let shown: Vec<f64> = [28, 27, 29]
            .iter()
            .map(|item| money(&fields[&format!("frm1701:txtPg1I{item}{column}")]))
            .collect();
        assert_eq!(
            shown, expected,
            "column {column} files the computed surcharge, interest and compromise"
        );
        assert_eq!(
            money(&fields[&format!("frm1701:txtPg1I30{column}")]),
            cents(expected.iter().sum::<f64>()),
            "column {column}: Item 30 adds them up"
        );
        assert_eq!(
            money(&fields[&format!("frm1701:txtPg1I31{column}TotalAmtPyble")]),
            cents(basic + expected.iter().sum::<f64>()),
            "column {column}: Item 31 follows"
        );
    }

    // One override changes only its item, its totals and the upload.
    let taxpayer = &declarations[0];
    let base = late.clone();
    let mut overridden = base.clone();
    taxpayer.set_override(&mut overridden, PenaltyComponent::Compromise, Some(500.0));
    compute(&mut overridden);
    let (before, after) = (base.field_map(), overridden.field_map());
    let changed: Vec<&String> = before
        .keys()
        .filter(|key| before.get(*key) != after.get(*key))
        .collect();
    assert_eq!(
        changed,
        [
            "frm1701:txtPg1I29A",
            "frm1701:txtPg1I30A",
            "frm1701:txtPg1I31ATotalAmtPyble",
            "frm1701:txtPg1I32AggregateAmtPyble",
        ]
    );
    assert_eq!(after["frm1701:txtPg1I29A"], "500.00");
    assert_ne!(
        overridden.submission_fingerprint(),
        base.submission_fingerprint()
    );
    // Clearing it brings the computed amount back.
    let mut cleared = overridden.clone();
    taxpayer.set_override(&mut cleared, PenaltyComponent::Compromise, None);
    compute(&mut cleared);
    assert_eq!(cleared.field_map(), before);

    // A draft stored while Items 27-29 were typed boxes (no penalty state)
    // files every amount it held, the zero surcharge too, and keeps its
    // stored shape.
    let mut stored = serde_json::to_value(&base).unwrap();
    for state in ["penalty", "spouse_penalty"] {
        stored.as_object_mut().unwrap().remove(state);
    }
    for (pointer, value) in [
        ("/computations/part_ii/27/taxpayer", 125.0),
        ("/computations/part_ii/28/taxpayer", 0.0),
        ("/computations/part_ii/29/taxpayer", 1_000.0),
    ] {
        *stored.pointer_mut(pointer).unwrap() = serde_json::json!(value);
    }
    let mut legacy: Form1701Draft = serde_json::from_value(stored).unwrap();
    assert_eq!(legacy.penalty.penalty_model_version, 0);
    compute(&mut legacy);
    assert_eq!(legacy.penalty, Default::default());
    let shape = serde_json::to_value(&legacy).unwrap();
    assert!(shape.get("penalty").is_none() && shape.get("spouse_penalty").is_none());
    let fields = legacy.field_map();
    assert_eq!(
        [
            &fields["frm1701:txtPg1I27A"],
            &fields["frm1701:txtPg1I28A"],
            &fields["frm1701:txtPg1I29A"]
        ],
        ["125.00", "0.00", "1,000.00"]
    );
    // Typing one (Edit penalties) keeps the others at what they held and
    // joins the model; blanking all three gives the computed amounts.
    let mut edited = legacy.clone();
    taxpayer.set_override(&mut edited, PenaltyComponent::Surcharge, Some(300.0));
    compute(&mut edited);
    let fields = edited.field_map();
    assert_eq!(
        [
            &fields["frm1701:txtPg1I27A"],
            &fields["frm1701:txtPg1I28A"],
            &fields["frm1701:txtPg1I29A"]
        ],
        ["125.00", "300.00", "1,000.00"]
    );
    assert_eq!(edited.penalty.penalty_model_version, 1);
    let mut back = edited.clone();
    for component in PenaltyComponent::ALL {
        taxpayer.set_override(&mut back, component, None);
    }
    compute(&mut back);
    assert_eq!(
        back.field_map()["frm1701:txtPg1I28A"],
        before["frm1701:txtPg1I28A"]
    );

    // The spouse's column applies only to a joint return; otherwise it
    // stays 0.00.
    let mut single = base.clone();
    single.civil_status = Some(Form1701CivilStatus::Single);
    compute(&mut single);
    assert!(!(declarations[1].applies)(&single));
    assert_eq!(single.field_map()["frm1701:txtPg1I29B"], "0.00");
    assert_eq!(single.spouse_penalty.computed_penalties, None);

    // Filed returns never recompute.
    for status in [
        FilingStatus::Sent,
        FilingStatus::Confirmed,
        FilingStatus::Paid,
    ] {
        let mut frozen = base.clone();
        frozen.lifecycle_mut().status = status;
        with_clock(day(2027, 1, 31), || frozen.recompute());
        assert_eq!(frozen.field_map(), base.field_map());
    }
}

/// Part IV Item 2: the page, the print and the upload read one value,
/// `spouse_rdo_code`, blank on the page and the print while there is none
/// (the upload files `000`) and once the spouse column closes. A joint save
/// file imported with its spouse's RDO keeps it in all three (PR 7 review).
#[test]
fn form_1701_spouse_rdo_agrees_on_page_print_and_upload() {
    let key = "frm1701:txtPg2I2SpouseRDOCode";
    let joint = sample_1701_joint();
    assert_eq!(joint.to_print_field_map()[key], "039");
    assert_eq!(joint.field_map()[key], "039");
    assert_eq!(joint.spouse_rdo_shown(), "039");
    // A joint save file in the official 837-field format whose spouse's RDO
    // is 040 (the spouse a single proprietor only, one Part IV item 3 mark).
    let mut source = joint.clone();
    source.spouse_also_compensation_earner = false;
    source.spouse.atc = Some(Form1701Atc::Ii012);
    source.employers[1] = Default::default();
    source.recompute();
    source.spouse_rdo_code = "040".into();
    let saved = source.to_official_xml_payload().unwrap();
    assert_eq!(saved.matches("txtPg2I2SpouseRDOCode=040").count(), 1);
    let imported = Form1701Draft::from_bir_xml_payload(&saved).unwrap();
    assert_eq!(imported.spouse_rdo_code, "040");
    assert_eq!(imported.spouse_rdo_shown(), "040", "page");
    assert_eq!(imported.to_print_field_map()[key], "040", "print");
    assert_eq!(imported.field_map()[key], "040", "upload");
    // None entered: blank on the page and the print, 000 uploaded.
    let mut none = joint.clone();
    none.spouse_rdo_code.clear();
    assert_eq!(none.spouse_rdo_shown(), "");
    assert_eq!(none.to_print_field_map()[key], "");
    assert_eq!(none.field_map()[key], "000");
    let mut single = joint.clone();
    single.civil_status = Some(Form1701CivilStatus::Single);
    assert_eq!(single.to_print_field_map()[key], "");
}

/// An imported 1701 save file keeps its Part VIII column A (PR 7,
/// preview.1): a save in the official format with item 1A = 500 files
/// 1A = 500 and item 3A (Sum of Items 1 and 2) = 500; before PR 7 every
/// Part VIII item was written 0.00. Column B stays 0.00.
#[test]
fn form_1701_imported_part_viii_files_as_imported() {
    let mut source = sample_1701_joint();
    source.spouse_also_compensation_earner = false;
    source.spouse.atc = Some(Form1701Atc::Ii012);
    source.employers[1] = Default::default();
    source.recompute();
    let official = source.to_official_xml_payload().unwrap();
    assert_eq!(official.matches("frm1701:txtPg4IPart8_1A=0.00").count(), 1);
    let saved = official.replace(
        "frm1701:txtPg4IPart8_1A=0.00",
        "frm1701:txtPg4IPart8_1A=500.00",
    );
    let mut imported = Form1701Draft::from_bir_xml_payload(&saved).unwrap();
    imported.recompute();
    let fields = imported.field_map();
    assert_eq!(fields["frm1701:txtPg4IPart8_1A"], "500.00");
    assert_eq!(fields["frm1701:txtPg4IPart8_3A"], "500.00");
    assert_eq!(fields["frm1701:txtPg4IPart8_1B"], "0.00");
}
