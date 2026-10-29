//! Regression snapshot of the deadline rows `deadlines_for_forms` resolves:
//! it pins what the code did, not an independent reading of the BIR
//! calendar. The dated sections were recorded from the code before the D8
//! deadline changes; D8 then removed the 24 2200M quarterly rows from them
//! (2200M is due per removal), and nothing else in them changed. Decision
//! P4-3 (2026-10-19) then removed 1606's 72 monthly rows (12 months, three
//! taxable years, both override sets) and added its one event row. The
//! event-based section was recorded after D8, from the code with D8
//! applied (2200M as needed, 2000OT added).
//!
//! A fixed profile (every form code the official calendar rules name) over
//! fixed taxable years, once with no overrides and once with a fixed set of
//! holiday overrides, rendered one row per line, then the event-based rows
//! (the as-needed obligations, which have no date and so no year), one per
//! line. Any change shows as a diff of
//! `deadline_golden/periodic_deadlines.txt`.
//!
//! Regenerate (only for an intended change, and review the diff) with
//! `BIR_UPDATE_DEADLINE_GOLDEN=1 cargo test -p bir-core --test periodic_deadlines_golden_test`.

use bir_core::calendar_rules::{DeadlineOverride, DeadlineResolver, canonical_form_code};
use chrono::NaiveDate;

const GOLDEN_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/deadline_golden/periodic_deadlines.txt"
);

fn date(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

/// The fixed profile: every canonical code the rules name, sorted.
fn profile_codes() -> Vec<String> {
    let mut codes: Vec<String> = DeadlineResolver::official_rules()
        .iter()
        .flat_map(|rule| {
            rule.form_nos
                .iter()
                .map(|no| canonical_form_code(no).to_string())
        })
        .collect();
    codes.sort();
    codes.dedup();
    codes
}

/// A fixed override set: three holidays that land on deadlines.
fn fixed_overrides() -> Vec<DeadlineOverride> {
    let all: Vec<String> = profile_codes();
    [
        ("Rizal Day", date(2026, 12, 30), date(2026, 12, 31)),
        ("Bonifacio Day", date(2026, 11, 30), date(2026, 12, 1)),
        ("Araw ng Kagitingan", date(2026, 4, 9), date(2026, 4, 10)),
    ]
    .into_iter()
    .map(|(title, original, adjusted)| DeadlineOverride {
        id: format!("golden-{original}"),
        title: title.to_string(),
        source_reference: "golden fixture".to_string(),
        affected_form_codes: all.clone(),
        original_deadline: original,
        adjusted_deadline: adjusted,
        affected_regions: Vec::new(),
        affected_taxpayer_types: Vec::new(),
        effective_from: None,
        effective_until: None,
        expires_at: None,
    })
    .collect()
}

fn render() -> String {
    let codes = profile_codes();
    let mut out = String::new();
    for (label, overrides) in [("none", Vec::new()), ("holidays", fixed_overrides())] {
        for year in 2025..=2027 {
            out.push_str(&format!("# overrides={label} taxable_year={year}\n"));
            for d in DeadlineResolver::deadlines_for_forms(&codes, year, &overrides) {
                let Some(final_date) = d.final_deadline_date() else {
                    continue;
                };
                out.push_str(&format!(
                    "{}|{:?}|{}|{}|{}|{:?}\n",
                    d.form_code,
                    d.period,
                    d.original_deadline_date()
                        .map(|x| x.to_string())
                        .unwrap_or_default(),
                    final_date,
                    d.period_start_string(),
                    d.status,
                ));
            }
        }
    }
    // The event-based rows the dated sections skip, recorded after D8 and
    // appended.
    out.push_str("# event-based obligations\n");
    for d in DeadlineResolver::event_based_obligations() {
        let bir_core::calendar_rules::DeadlineKind::EventBased {
            trigger,
            statutory_window,
        } = &d.deadline
        else {
            panic!("{} event row is dated", d.form_code);
        };
        out.push_str(&format!(
            "{}|{}|{}|{}|{:?}\n",
            d.form_code, d.display_form_no, trigger, statutory_window, d.status
        ));
    }
    out
}

#[test]
fn periodic_deadline_rows_match_the_golden() {
    let rendered = render();
    if std::env::var_os("BIR_UPDATE_DEADLINE_GOLDEN").is_some() {
        std::fs::write(GOLDEN_PATH, &rendered).unwrap();
    }
    let golden = std::fs::read_to_string(GOLDEN_PATH).expect("golden file");
    assert!(
        golden == rendered,
        "periodic deadline rows changed; diff {GOLDEN_PATH} against a regenerated copy"
    );
}

/// Decision P4-3: the golden holds no 1606 monthly row, and 1606 is one
/// event-based row (the 72 monthly rows it held went).
#[test]
fn the_golden_holds_1606_as_an_event_row_only() {
    let golden = std::fs::read_to_string(GOLDEN_PATH).expect("golden file");
    assert!(!golden.lines().any(|line| line.starts_with("1606|Monthly")));
    assert_eq!(
        golden
            .lines()
            .filter(|line| line.starts_with("1606|1606|"))
            .count(),
        1
    );
}
