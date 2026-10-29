//! The shared Surcharge / Interest / Compromise model, checked on every form
//! that declares penalty items, starting from its dummy sample.
//!
//! Each form's `*_penalty_items_follow_the_shared_model` test runs
//! [`check`]: the sample, re-keyed to the test TIN 111-111-114-00000 (never
//! real data), filed late on 13 October 2026, computes the penalties the
//! configured rates give; overriding one component changes only that item,
//! the totals, the XML value and the queue fingerprint; clearing it returns
//! to the computed value; and a draft saved while the items were manual-only
//! keeps its non-zero hand-typed amounts as overrides.
//!
//! Rates, as `PenaltyConfig` carries them for the standard (non-micro/small)
//! class: surcharge 25% (NIRC Sec. 248(A)); interest 12% a year (NIRC Sec.
//! 249(B) as amended by RA 10963); compromise from the RMO 7-2015 Annex A
//! schedule (`compromise.rs`).

use std::collections::BTreeMap;

use bir_core::filing_queue::QueueAuthSource;
use bir_core::forms::FilingStatus;
use bir_core::forms::penalty_items::{PenaltyItems, PenaltyRounding, with_clock};
use bir_core::forms::queueable::{QueueableForm, QueueableKind};
use bir_core::penalties::compromise::lookup_compromise;
use bir_core::penalties::{PenaltyComponent, PenaltyProfile};
use chrono::NaiveDate;
use serde_json::Value;

/// The day the late samples are filed and paid.
pub const FILED: (i32, u32, u32) = (2026, 10, 13);

pub fn day(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

fn filed() -> NaiveDate {
    day(FILED.0, FILED.1, FILED.2)
}

/// Where a form's deadline comes from.
pub enum Deadline {
    /// The app's calendar resolves this deadline for the sample's period.
    Calendar(NaiveDate),
    /// Event-based return: the calendar has no deadline, nothing is computed
    /// and the items are overrides only. To prove the declaration still
    /// feeds the engine, the check then supplies this resolved deadline the
    /// way an override from the app would.
    None { assumed: NaiveDate },
    /// Decision D8 (2026-10-18, the excise returns): the due date is the one
    /// the form reads from what the filer typed (`typed_due_date`), here
    /// `due` for the sample; a draft stored before the form measured from
    /// it computes nothing until it joins.
    Typed { due: NaiveDate },
}

const KEYS: [&str; 5] = [
    "penalty_model_version",
    "penalty_due_date",
    "penalty_taxpayer_class",
    "computed_penalties",
    "penalty_overrides",
];

fn strip_penalty_state(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for key in KEYS {
                map.remove(key);
            }
            map.values_mut().for_each(strip_penalty_state);
        }
        Value::Array(items) => items.iter_mut().for_each(strip_penalty_state),
        _ => {}
    }
}

fn json_at<'a>(value: &'a Value, path: &str) -> &'a Value {
    path.split('.').fold(value, |value, key| &value[key])
}

fn rebuild<F: QueueableForm>(draft: &F, edit: impl FnOnce(&mut Value)) -> F {
    let mut json = serde_json::to_value(draft).unwrap();
    edit(&mut json);
    serde_json::from_value(json).unwrap()
}

/// The sample, re-keyed to the test TIN 111-111-114-00000.
fn with_test_tin<F: QueueableForm>(draft: F) -> F {
    rebuild(&draft, |json| {
        if json.get("tin").is_some() {
            json["tin"] = Value::String("11111111400000".into());
        }
    })
}

fn rounded(items: &PenaltyItems<impl Sized>, value: f64) -> f64 {
    match items.rounding {
        PenaltyRounding::Cents => (value * 100.0).round() / 100.0,
        PenaltyRounding::WholePesos => value.round(),
    }
}

fn compute<F: QueueableForm>(draft: &mut F) {
    with_clock(filed(), || draft.compute());
}

fn amount_payable<F: QueueableForm>(draft: &F) -> f64 {
    draft.amount_payable().unwrap_or(0.0)
}

/// Every declaration names three distinct draft fields that hold its
/// penalty items, and never a field that only looks like one.
pub fn check_declaration<F: QueueableForm>(sample: &F) {
    let declarations = F::penalty_items();
    assert!(
        !declarations.is_empty(),
        "{} declares no penalty items",
        F::FORM_CODE
    );
    let json = serde_json::to_value(sample).unwrap();
    for items in &declarations {
        let [s, i, c] = items.fields;
        assert!(
            s != i && i != c && s != c,
            "{}: fields {:?}",
            F::FORM_CODE,
            items.fields
        );
        for ((field, label), word) in
            items
                .fields
                .iter()
                .zip(items.labels)
                .zip(["surcharge", "interest", "compromise"])
        {
            let leaf = field.rsplit('.').next().unwrap();
            let named = leaf == word
                || leaf.ends_with(&format!("_{word}"))
                || (word == "interest" && leaf == "interest_penalty");
            assert!(named, "{}: `{field}` is not a {word} item", F::FORM_CODE);
            assert!(
                label.to_lowercase().contains(word),
                "{}: label {label:?} for `{field}`",
                F::FORM_CODE
            );
            assert!(
                json_at(&json, field).is_number() || json_at(&json, field).is_object(),
                "{}: `{field}` is not a stored draft field",
                F::FORM_CODE
            );
        }
        // Writing through the declaration lands on exactly that stored field.
        for (component, field) in PenaltyComponent::ALL.into_iter().zip(items.fields) {
            let mut probe = sample.clone();
            (items.set)(&mut probe, component, 4_321.0);
            assert_eq!((items.get)(&probe, component), 4_321.0);
            let after = serde_json::to_value(&probe).unwrap();
            assert_ne!(
                json_at(&after, field),
                json_at(&json, field),
                "{}: setting {component:?} did not change `{field}`",
                F::FORM_CODE
            );
            for other in items.fields.iter().filter(|other| **other != field) {
                assert_eq!(json_at(&after, other), json_at(&json, other));
            }
        }
        let [cs, ci, cc] = items.controls;
        assert!(cs != ci && ci != cc && cs != cc);
        let fields = sample.field_map();
        for control in items.controls {
            assert!(
                fields.contains_key(control),
                "{}: control {control} is not in the field map",
                F::FORM_CODE
            );
        }
    }
}

fn changed_keys(a: &BTreeMap<String, String>, b: &BTreeMap<String, String>) -> Vec<String> {
    a.keys()
        .chain(b.keys())
        .filter(|key| a.get(*key) != b.get(*key))
        .cloned()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// The whole check for one form's sample (see the module doc).
pub fn check<F: QueueableForm>(sample: fn() -> F, deadline: Deadline) {
    let code = F::FORM_CODE;
    let original = with_test_tin(sample());
    check_declaration(&original);
    let declarations = F::penalty_items();
    let applying: Vec<_> = declarations
        .iter()
        .filter(|items| (items.applies)(&original))
        .collect();
    assert!(
        !applying.is_empty(),
        "{code}: no penalty items apply to the sample"
    );

    // ---- Late sample: computed from the configured rates.
    let mut late = original.clone();
    for items in &declarations {
        (items.state_mut)(&mut late).penalty_overrides = Default::default();
    }
    if let Deadline::None { assumed } = deadline {
        // No deadline in the calendar: nothing is computed, nothing is filed.
        let mut manual = late.clone();
        compute(&mut manual);
        for items in &applying {
            assert_eq!(items.due_date(&manual), None, "{code}: invented a deadline");
            assert_eq!((items.state)(&manual).computed_penalties, None);
            assert_eq!(
                items.filed(&manual),
                [0.0; 3],
                "{code}: filed without an override"
            );
        }
        for items in &declarations {
            (items.state_mut)(&mut late).penalty_due_date = Some(assumed);
        }
    }
    if let Deadline::Typed { due } = deadline {
        // Stored before D8 (the model version every other form is on):
        // nothing is computed, not even from a deadline the app resolved.
        let mut stored = late.clone();
        for items in &declarations {
            let state = (items.state_mut)(&mut stored);
            state.penalty_model_version = bir_core::forms::penalty_items::PENALTY_MODEL_VERSION;
            state.penalty_due_date = Some(day(2020, 1, 1));
        }
        compute(&mut stored);
        for items in &applying {
            assert!(items.typed_due_date.is_some(), "{code}: typed due date");
            assert_eq!(items.due_date(&stored), None, "{code}: stored before D8");
            assert_eq!((items.state)(&stored).computed_penalties, None);
            assert_eq!(items.filed(&stored), [0.0; 3], "{code}: stored before D8");
        }
        // A new return, or one that joins, runs from the typed date alone.
        for items in &declarations {
            items.join_typed_due_date(&mut late);
            (items.state_mut)(&mut late).penalty_due_date = Some(day(2020, 1, 1));
        }
        for items in &applying {
            assert_eq!(items.due_date(&late), Some(due), "{code}: typed due date");
        }
    }
    compute(&mut late);
    let mut any_positive = false;
    for items in &applying {
        let computed = (items.state)(&late)
            .computed_penalties
            .clone()
            .unwrap_or_else(|| panic!("{code}: a late sample computes its penalties"));
        let inputs = &computed.inputs;
        if let Deadline::Calendar(due) | Deadline::Typed { due } = deadline {
            assert_eq!(inputs.due_date, due, "{code}: deadline");
        }
        assert_eq!(inputs.filing_date, filed());
        assert!(inputs.days_late > 0, "{code}: the sample is filed late");
        assert_eq!(inputs.schedule, items.schedule);
        let basic = (items.basic_tax_due)(&late).max(0.0);
        assert_eq!(
            inputs.basic_tax_due,
            (basic * 100.0).round() / 100.0,
            "{code}: basic tax due"
        );
        let expected = if inputs.basic_tax_due > 0.0 {
            any_positive = true;
            [
                rounded(items, inputs.basic_tax_due * 0.25),
                rounded(
                    items,
                    inputs.basic_tax_due * 0.12 * inputs.days_late as f64 / 365.0,
                ),
                rounded(
                    items,
                    lookup_compromise(inputs.basic_tax_due, 0.0, items.schedule),
                ),
            ]
        } else {
            [0.0; 3]
        };
        assert_eq!(
            [computed.surcharge, computed.interest, computed.compromise],
            expected,
            "{code}{}: computed from the configured rates on {} days",
            items.party.map(|p| format!(" ({p})")).unwrap_or_default(),
            inputs.days_late
        );
        assert_eq!(
            items.filed(&late),
            expected,
            "{code}: the items file the computed values"
        );
    }
    if !any_positive {
        // Only a form whose sample has no tax still due gets here.
        assert_eq!(code, "2200S", "{code}: the sample has tax still due");
    }

    // ---- Override one component: only it changes.
    let items = applying[0];
    let base = late.clone();
    let base_fields = base.field_map();
    let [base_s, base_i, base_c] = items.filed(&base);
    let mut overridden = base.clone();
    items.set_override(&mut overridden, PenaltyComponent::Compromise, Some(500.0));
    (items.state_mut)(&mut overridden).penalty_overrides.note =
        "RDO approval ref. 2026-0042".into();
    compute(&mut overridden);
    assert_eq!(
        items.filed(&overridden),
        [base_s, base_i, 500.0],
        "{code}: override"
    );
    assert_eq!(
        (items.state)(&overridden).computed_penalties,
        (items.state)(&base).computed_penalties,
        "{code}: the computed values are kept next to the override"
    );
    // 2200-AN's compute22 adds Items 21A-21C and their total 21D to Item 22,
    // so the official page counts each penalty twice there.
    let times = if code == "2200AN" { 2.0 } else { 1.0 };
    let delta = amount_payable(&overridden) - amount_payable(&base);
    assert!(
        (delta - times * (500.0 - base_c)).abs() < 1.0,
        "{code}: total payable moved by {delta}, expected {}",
        times * (500.0 - base_c)
    );
    let fields = overridden.field_map();
    let changed = changed_keys(&base_fields, &fields);
    let [cs, ci, cc] = items.controls;
    if base_c != 500.0 {
        assert!(
            changed.iter().any(|key| key == cc),
            "{code}: compromise control {cc} did not change; changed: {changed:?}"
        );
        assert!(fields[cc].contains("500"), "{code}: {cc} = {}", fields[cc]);
        assert_ne!(
            overridden.submission_fingerprint(),
            base.submission_fingerprint()
        );
    }
    assert!(
        !changed.iter().any(|key| key == cs || key == ci),
        "{code}: {changed:?}"
    );
    let restored: F = serde_json::from_str(&serde_json::to_string(&overridden).unwrap()).unwrap();
    assert_eq!(
        (items.state)(&restored).penalty_overrides,
        (items.state)(&overridden).penalty_overrides
    );

    // ---- Clearing the override returns to the computed value.
    let mut cleared = overridden.clone();
    items.set_override(&mut cleared, PenaltyComponent::Compromise, None);
    compute(&mut cleared);
    assert_eq!(items.filed(&cleared), [base_s, base_i, base_c]);
    assert_eq!(cleared.field_map(), base_fields);

    // ---- A draft saved while the items were manual-only keeps them.
    let manual = match items.rounding {
        PenaltyRounding::Cents => [125.25, 0.0, 1_000.0],
        PenaltyRounding::WholePesos => [125.0, 0.0, 1_000.0],
    };
    let mut typed = base.clone();
    for (component, value) in PenaltyComponent::ALL.into_iter().zip(manual) {
        (items.set)(&mut typed, component, value);
    }
    let mut legacy: F = rebuild(&typed, strip_penalty_state);
    assert_eq!((items.state)(&legacy).penalty_model_version, 0);
    compute(&mut legacy);
    let overrides = &(items.state)(&legacy).penalty_overrides;
    assert_eq!(
        overrides.surcharge,
        Some(manual[0]),
        "{code}: legacy surcharge"
    );
    assert_eq!(
        overrides.interest, None,
        "{code}: a zero is not an override"
    );
    assert_eq!(
        overrides.compromise,
        Some(manual[2]),
        "{code}: legacy compromise"
    );
    // The zero item is computed again; an event-based return has no
    // deadline once the assumed one is gone, so it computes nothing, and a
    // D8 draft saved manual-only predates its typed due date.
    let interest = match deadline {
        Deadline::Calendar(_) => base_i,
        Deadline::None { .. } | Deadline::Typed { .. } => 0.0,
    };
    assert_eq!(
        items.filed(&legacy),
        [manual[0], interest, manual[2]],
        "{code}: legacy values"
    );

    // ---- Queued: the clock moves only at revalidation, and a change there
    // fails closed; an overridden component does not move. A typed due date
    // moves the same way once there is a tax still due to charge.
    if matches!(deadline, Deadline::Calendar(_))
        || (matches!(deadline, Deadline::Typed { .. }) && any_positive)
    {
        let mut queued = base.clone();
        let queue = with_clock(filed(), || queued.queue(QueueAuthSource::Gui));
        if let Err(errors) = queue {
            panic!("{code}: the late sample queues: {errors:?}");
        }
        let errors = with_clock(day(2026, 10, 14), || {
            queued.revalidate_queued_before_submission()
        })
        .expect_err("changed computed penalties must require review");
        assert!(
            errors
                .iter()
                .any(|(field, _)| field == "computed_penalties"),
            "{code}: {errors:?}"
        );
        assert_eq!(queued.lifecycle().status, FilingStatus::Draft);

        let mut pinned = base.clone();
        for items in &declarations {
            if (items.applies)(&pinned) {
                let filed_now = items.filed(&pinned);
                for (component, value) in PenaltyComponent::ALL.into_iter().zip(filed_now) {
                    items.set_override(&mut pinned, component, Some(value));
                }
            }
        }
        with_clock(filed(), || pinned.queue(QueueAuthSource::Gui)).unwrap();
        with_clock(day(2026, 10, 14), || {
            pinned.revalidate_queued_before_submission()
        })
        .unwrap_or_else(|errors| panic!("{code}: overrides do not move: {errors:?}"));
        assert_eq!(pinned.lifecycle().status, FilingStatus::Queued);
    }

    // ---- Submitted, Confirmed and Paid returns never recompute.
    for status in [
        FilingStatus::Sent,
        FilingStatus::Confirmed,
        FilingStatus::Paid,
    ] {
        let mut frozen = base.clone();
        frozen.lifecycle_mut().status = status;
        with_clock(day(2027, 1, 31), || frozen.compute());
        assert_eq!(items.filed(&frozen), items.filed(&base), "{code}: frozen");
        assert_eq!(
            (items.state)(&frozen).computed_penalties,
            (items.state)(&base).computed_penalties
        );
    }
}

/// A form's official-XML importer.
pub type Import<F> = fn(&str) -> Result<F, Vec<(String, String)>>;

/// An imported official return keeps the penalties it was prepared with:
/// every item becomes an override, so the source amounts round-trip even
/// though the period is late.
pub fn check_import<F: QueueableForm>(sample: fn() -> F, import: Import<F>) {
    let code = F::FORM_CODE;
    let source = sample();
    let fields = source.field_map();
    let payload = source
        .official_payload()
        .unwrap_or_else(|errors| panic!("{code}: {errors:?}"));
    let mut imported = import(&payload).unwrap_or_else(|errors| panic!("{code}: {errors:?}"));
    for items in F::penalty_items() {
        if !(items.applies)(&imported) {
            continue;
        }
        let state = (items.state)(&imported);
        let [s, i, c] = items.filed(&source);
        assert_eq!(
            state.penalty_overrides.surcharge,
            Some(s),
            "{code}: imported surcharge"
        );
        assert_eq!(
            state.penalty_overrides.interest,
            Some(i),
            "{code}: imported interest"
        );
        assert_eq!(
            state.penalty_overrides.compromise,
            Some(c),
            "{code}: imported compromise"
        );
    }
    compute(&mut imported);
    assert_eq!(
        imported.field_map(),
        fields,
        "{code}: the import round-trips"
    );
}

/// Every form that declares penalty items has this check, and every form
/// listed here declares them. 1701Q (PR 5) and 1701 (PR 7) run their own
/// version of the check (crates/bir-core/tests/sample_payloads/form_1701q.rs
/// `form_1701q_penalty_items_follow_the_shared_model` and form_1701.rs
/// `form_1701_penalty_items_follow_the_shared_model`): their items are kept
/// by item number, not by a field named after them.
pub const WIRED: &[&str] = &[
    "0605", "0619E", "0619F", "1600PT", "1600VT", "1600WP", "1601EQ", "1601FQ", "1602Q", "1603Q",
    "1606", "1700", "1701", "1701A", "1701MS", "1701Q", "1702MX", "1702Q", "1702RT", "1706",
    "1707", "1707A", "1800", "1801", "2000", "2000OT", "2200A", "2200AN", "2200C", "2200M",
    "2200P", "2200S", "2200T", "2550M", "2552", "2553",
];

#[test]
fn every_form_with_penalty_items_is_checked() {
    let declared: Vec<&str> = QueueableKind::ALL
        .iter()
        .filter(|kind| !kind.penalty_fields().is_empty())
        .map(|kind| kind.form_code())
        .collect();
    assert_eq!(declared, {
        let mut wired = WIRED.to_vec();
        wired.sort_by_key(|code| {
            QueueableKind::ALL
                .iter()
                .position(|kind| kind.form_code() == *code)
                .unwrap()
        });
        wired
    });
}

/// The fields that look like penalty items but are not.
#[test]
fn look_alike_fields_are_never_declared() {
    use bir_core::forms::{
        form_0619f::Form0619FDraft, form_1602q::Form1602QDraft, form_1801::Form1801Draft,
    };
    let fields = |items: Vec<PenaltyItems<_>>| -> Vec<&'static str> {
        items.iter().flat_map(|items| items.fields).collect()
    };
    // 1602Q: `interest` is the interest income on deposits; Item 25 is
    // `interest_penalty`.
    let f1602q = fields(<Form1602QDraft as QueueableForm>::penalty_items());
    assert!(f1602q.contains(&"interest_penalty") && !f1602q.contains(&"interest"));
    // 1801: `business_interest` is an estate asset.
    let f1801: Vec<&str> = <Form1801Draft as QueueableForm>::penalty_items()
        .iter()
        .flat_map(|items| items.fields)
        .collect();
    assert!(!f1801.contains(&"business_interest") && f1801.contains(&"interest"));
    // 0619F: Item 13 is the final tax withheld on interest.
    let f0619f: Vec<&str> = <Form0619FDraft as QueueableForm>::penalty_items()
        .iter()
        .flat_map(|items| items.fields)
        .collect();
    assert!(!f0619f.contains(&"item_13_interest_final_tax_withheld"));
    assert!(f0619f.contains(&"item_18b_interest"));
    // The withholding schedule is for returns that remit tax withheld from
    // others; every other wired return files on the standard schedule.
    let withholding = [
        "0619E", "0619F", "1600PT", "1600VT", "1600WP", "1601EQ", "1601FQ", "1602Q", "1603Q",
        "1606",
    ];
    for kind in QueueableKind::ALL {
        for schedule in kind.penalty_schedules() {
            let expected = if withholding.contains(&kind.form_code()) {
                PenaltyProfile::Withholding
            } else {
                PenaltyProfile::StandardFiling
            };
            assert_eq!(schedule, expected, "{}", kind.form_code());
        }
    }
}

/// PR 11 golden, extended by maintainer decision (1800/1801 penalties run from
/// the due date): a typed due date is declared by
/// the seven excise returns (D8) and by 1800 and 1801 (P4-4), and by no
/// other form, so every other form's penalties keep their calendar due date
/// (the `if let` in `PenaltyItems::due_date` is not taken); each form's
/// `*_penalty_items_follow_the_shared_model` still pins its computed
/// amounts.
#[test]
fn only_the_excise_and_donor_estate_returns_measure_penalties_from_a_typed_date() {
    let mut typed = Vec::new();
    for kind in QueueableKind::ALL {
        bir_core::with_queueable_kind!(*kind, F => {
            for items in F::penalty_items() {
                if items.typed_due_date.is_some() {
                    typed.push(F::FORM_CODE);
                }
            }
        });
    }
    typed.sort_unstable();
    assert_eq!(
        typed,
        [
            "1800", "1801", "2200A", "2200AN", "2200C", "2200M", "2200P", "2200S", "2200T"
        ]
    );
    assert!(
        bir_core::forms::form_1601c::Form1601CDraft::penalty_items()
            .typed_due_date
            .is_none()
    );
}
