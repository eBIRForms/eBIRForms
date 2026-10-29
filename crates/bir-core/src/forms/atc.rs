//! Alphanumeric Tax Code (ATC) reference data for percentage tax forms.

pub struct AtcEntry {
    pub code: &'static str,
    pub description: &'static str,
    pub rate: f64,
}

/// Period-specific resolution for a 2551Q Schedule 1 rate.
///
/// The January 2018 form's printed reference table remains the base registry,
/// but CREATE temporarily reduced Section 116 (PT010) from 3% to 1% for
/// receipts from July 2020 through June 2023. A fiscal quarter that crosses
/// either statutory boundary cannot be represented by one aggregate amount
/// and one rate, so callers must fail closed instead of guessing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AtcRateResolution {
    Single(f64),
    RequiresPeriodSplit,
}

pub const ATC_TABLE_2551Q: &[AtcEntry] = &[
    AtcEntry {
        code: "PT010",
        description: "Persons exempt from VAT under Sec. 109(BB) [Sec. 116]",
        rate: 0.03,
    },
    AtcEntry {
        code: "PT040",
        description: "Domestic carriers and keepers of garages [Sec. 117]",
        rate: 0.03,
    },
    AtcEntry {
        code: "PT041",
        description: "International Carriers [Sec. 118]",
        rate: 0.03,
    },
    AtcEntry {
        code: "PT060",
        description: "Franchises on gas and water utilities [Sec. 119]",
        rate: 0.02,
    },
    AtcEntry {
        code: "PT070",
        description: "Franchises on radio/TV broadcasting companies whose annual gross receipts do not exceed P10 M [Sec. 119]",
        rate: 0.03,
    },
    AtcEntry {
        code: "PT090",
        description: "Overseas dispatch, message or conversation originating from the Philippines [Sec. 120]",
        rate: 0.10,
    },
    AtcEntry {
        code: "PT140",
        description: "Cockpits [Sec. 125]",
        rate: 0.18,
    },
    AtcEntry {
        code: "PT150",
        description: "Tax on amusement places, such as cabarets, night and day clubs, videoke bars, karaoke bars, karaoke television, karaoke boxes, music lounges and other similar establishments [Sec. 125]",
        rate: 0.18,
    },
    AtcEntry {
        code: "PT160",
        description: "Boxing Exhibition [Sec. 125]",
        rate: 0.10,
    },
    AtcEntry {
        code: "PT170",
        description: "Professional Basketball Games [Sec. 125]",
        rate: 0.15,
    },
    AtcEntry {
        code: "PT180",
        description: "Jai-alai and Race Tracks [Sec. 125]",
        rate: 0.30,
    },
    AtcEntry {
        code: "PT105",
        description: "- Maturity period is five (5) years or less",
        rate: 0.05,
    },
    AtcEntry {
        code: "PT101",
        description: "- Maturity period is more than five (5) years",
        rate: 0.01,
    },
    AtcEntry {
        code: "PT102",
        description: "2) On dividends and equity shares and net income of subsidiaries",
        rate: 0.00,
    },
    AtcEntry {
        code: "PT103",
        description: "3) On royalties, rentals of property, real or personal, profits from exchange and all other gross income",
        rate: 0.07,
    },
    AtcEntry {
        code: "PT104",
        description: "4) On net trading gains within the taxable year on foreign currency, debt securities, derivatives and other financial instruments",
        rate: 0.07,
    },
    AtcEntry {
        code: "PT113",
        description: "- Maturity period is five (5) years or less",
        rate: 0.05,
    },
    AtcEntry {
        code: "PT114",
        description: "- Maturity period is more than five (5) years",
        rate: 0.01,
    },
    AtcEntry {
        code: "PT115",
        description: "2) From all other items treated as gross income under the code",
        rate: 0.05,
    },
    AtcEntry {
        code: "PT120",
        description: "Life Insurance Premiums [Sec. 123]",
        rate: 0.02,
    },
    AtcEntry {
        code: "PT130",
        description: "1) Insurance Agents",
        rate: 0.04,
    },
    AtcEntry {
        code: "PT132",
        description: "2) Owners of property obtaining insurance directly with foreign insurance companies",
        rate: 0.05,
    },
];

/// Looks up an ATC entry by code. Returns None if not found.
pub fn find_atc(code: &str) -> Option<&'static AtcEntry> {
    ATC_TABLE_2551Q.iter().find(|e| e.code == code)
}

pub fn resolve_2551q_atc_rate(
    code: &str,
    taxable_year: u16,
    quarter: u8,
    year_end_month: u8,
) -> Option<AtcRateResolution> {
    let entry = find_atc(code)?;
    if code != "PT010" || !(1..=4).contains(&quarter) || !(1..=12).contains(&year_end_month) {
        return Some(AtcRateResolution::Single(entry.rate));
    }

    // Absolute month indexes make calendar and fiscal quarters use the same
    // boundary logic. `taxable_year/year_end_month` is the end of Q4.
    let fiscal_year_end = i32::from(taxable_year) * 12 + i32::from(year_end_month) - 1;
    let quarter_end = fiscal_year_end - i32::from(4 - quarter) * 3;
    let quarter_start = quarter_end - 2;
    let reduced_start = 2020 * 12 + 6; // July 2020
    let reduced_end = 2023 * 12 + 5; // June 2023

    if quarter_start >= reduced_start && quarter_end <= reduced_end {
        Some(AtcRateResolution::Single(0.01))
    } else if quarter_end < reduced_start || quarter_start > reduced_end {
        Some(AtcRateResolution::Single(entry.rate))
    } else {
        Some(AtcRateResolution::RequiresPeriodSplit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn form_2551q_registry_matches_the_january_2018_page_two_table() {
        let expected = [
            (
                "PT010",
                "Persons exempt from VAT under Sec. 109(BB) [Sec. 116]",
                0.03,
            ),
            (
                "PT040",
                "Domestic carriers and keepers of garages [Sec. 117]",
                0.03,
            ),
            ("PT041", "International Carriers [Sec. 118]", 0.03),
            (
                "PT060",
                "Franchises on gas and water utilities [Sec. 119]",
                0.02,
            ),
            (
                "PT070",
                "Franchises on radio/TV broadcasting companies whose annual gross receipts do not exceed P10 M [Sec. 119]",
                0.03,
            ),
            (
                "PT090",
                "Overseas dispatch, message or conversation originating from the Philippines [Sec. 120]",
                0.10,
            ),
            ("PT140", "Cockpits [Sec. 125]", 0.18),
            (
                "PT150",
                "Tax on amusement places, such as cabarets, night and day clubs, videoke bars, karaoke bars, karaoke television, karaoke boxes, music lounges and other similar establishments [Sec. 125]",
                0.18,
            ),
            ("PT160", "Boxing Exhibition [Sec. 125]", 0.10),
            ("PT170", "Professional Basketball Games [Sec. 125]", 0.15),
            ("PT180", "Jai-alai and Race Tracks [Sec. 125]", 0.30),
            ("PT105", "- Maturity period is five (5) years or less", 0.05),
            (
                "PT101",
                "- Maturity period is more than five (5) years",
                0.01,
            ),
            (
                "PT102",
                "2) On dividends and equity shares and net income of subsidiaries",
                0.00,
            ),
            (
                "PT103",
                "3) On royalties, rentals of property, real or personal, profits from exchange and all other gross income",
                0.07,
            ),
            (
                "PT104",
                "4) On net trading gains within the taxable year on foreign currency, debt securities, derivatives and other financial instruments",
                0.07,
            ),
            ("PT113", "- Maturity period is five (5) years or less", 0.05),
            (
                "PT114",
                "- Maturity period is more than five (5) years",
                0.01,
            ),
            (
                "PT115",
                "2) From all other items treated as gross income under the code",
                0.05,
            ),
            ("PT120", "Life Insurance Premiums [Sec. 123]", 0.02),
            ("PT130", "1) Insurance Agents", 0.04),
            (
                "PT132",
                "2) Owners of property obtaining insurance directly with foreign insurance companies",
                0.05,
            ),
        ];

        assert_eq!(ATC_TABLE_2551Q.len(), expected.len());
        let mut codes = HashSet::new();
        for ((expected_code, expected_description, expected_rate), entry) in
            expected.iter().zip(ATC_TABLE_2551Q)
        {
            assert_eq!(entry.code, *expected_code);
            assert_eq!(entry.description, *expected_description);
            assert!((entry.rate - expected_rate).abs() < f64::EPSILON);
            assert!(
                codes.insert(entry.code),
                "duplicate ATC code {}",
                entry.code
            );
        }
    }

    #[test]
    fn retired_or_invented_codes_are_not_valid_for_the_2018_form() {
        for code in ["PT011", "PT019", "PT050", "PT080", "PT100", "PT110"] {
            assert!(find_atc(code).is_none(), "{code} must not be registered");
        }
    }

    #[test]
    fn pt010_rate_tracks_the_create_act_window_for_calendar_quarters() {
        for (year, quarter, expected) in [
            (2020, 2, 0.03),
            (2020, 3, 0.01),
            (2023, 2, 0.01),
            (2023, 3, 0.03),
        ] {
            assert_eq!(
                resolve_2551q_atc_rate("PT010", year, quarter, 12),
                Some(AtcRateResolution::Single(expected))
            );
        }
    }

    #[test]
    fn pt010_fiscal_quarters_crossing_a_rate_boundary_require_a_split() {
        assert_eq!(
            resolve_2551q_atc_rate("PT010", 2020, 4, 8),
            Some(AtcRateResolution::RequiresPeriodSplit),
            "June-August 2020 crosses the July 1 rate boundary"
        );
        assert_eq!(
            resolve_2551q_atc_rate("PT010", 2023, 4, 8),
            Some(AtcRateResolution::RequiresPeriodSplit),
            "June-August 2023 crosses the July 1 rate boundary"
        );
    }

    #[test]
    fn the_temporary_reduction_does_not_change_other_atcs() {
        assert_eq!(
            resolve_2551q_atc_rate("PT060", 2021, 1, 12),
            Some(AtcRateResolution::Single(0.02))
        );
    }
}
