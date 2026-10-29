//! Form 0605 (Payment Form, July 1999) on the generic submission path.
//!
//! Ported from the official `BIR-Form0605.hta` (eBIRForms 7.9.6.2.1): the
//! compute chain (`computePenalties`, `computeOfTax`), the Item 6 / Item 8
//! popups (`TaxTypeCodeList`, `ATCList`, `getATCCode`, `getTaxTypeCode`),
//! `validate()` / `initialValidateBeforeSave()` with their exact alert texts,
//! and `saveXMLsubmit` through [`crate::official_xml`].
//!
//! The official layout is the page as it is written in the HTA. When the
//! page loads it adds one `TaxTypeCode<n>` radio per Item 8 tax type, and
//! choosing a tax type adds one `AtcCode<n>` radio per Item 6 ATC; all of
//! them sit in the form just before `txtFinalFlag`, so `saveXMLsubmit`
//! writes them there. [`QueueableForm::official_payload`] adds them the same
//! way.

use super::penalty_items::PenaltyItems;
use std::collections::BTreeMap;

use super::form_0605::{
    Form0605ApprovalSelection, Form0605Date, Form0605Draft, Form0605FilingBasis,
    Form0605IndexedCode, Form0605MannerOfPayment, Form0605TaxpayerClassification,
    Form0605TypeOfPayment,
};
use super::official_inputs::has_cent_precision;
use super::queueable::{QueueableForm, SubmissionLifecycle};
use super::{FilingPeriod, FormValidator};
use crate::official_xml::{official_amount, parse_official_amount};

/// Rule-package id of the official layout.
pub const FORM_0605_LAYOUT_ID: &str = "0605-v2003";
/// Official `formType` and PROD SFTP folder (`ftpTargetFolder.PROD['0605']`).
pub const FORM_0605_FORM_TYPE: &str = "0605";

/// Item 8 tax types in `xml/taxTypeCodes.xml` order (`TaxTypeCode<n>` is n = index + 1).
pub const FORM_0605_TAX_TYPES: &[(&str, &str)] = &[
    ("CG", "CAPITAL GAINS TAX-Real Property"),
    ("CS", "CAPITAL GAINS TAX-Stocks"),
    ("DN", "DONORS TAX"),
    ("DO", "DOCUMENTARY STAMP TAX-ONE TIME"),
    ("DS", "DOCUMENTARY STAMP TAX"),
    ("ES", "ESTATE TAX"),
    ("ET", "ENERGY TAX"),
    ("IE", "IMPROPERLY ACCUMULATED EARNINGS"),
    ("IT", "INCOME TAX"),
    ("MC", "MISCELLANEOUS TAX"),
    ("PM", "PERCENTAGE TAX-MONTHLY"),
    ("PT", "PERCENTAGE TAX-QUARTERLY"),
    ("QP", "QUALIFYING FEES-PAGCOR"),
    ("RF", "REGISTRATION FEE"),
    ("SL", "PERCENTAGE TAX-SPECIAL LAWS"),
    ("SO", "PERCENTAGE TAX-STOCKS(IPO)"),
    ("ST", "PERCENTAGE TAX-STOCKS"),
    ("TR", "TRAVEL TAX-PTA"),
    ("VT", "VALUE ADDED TAX"),
    (
        "WB",
        "WITHHOLDING TAX-BANKS AND OTHER FINANCIAL INSTITUTIONS",
    ),
    ("WC", "WITHHOLDING TAX-COMPENSATION"),
    ("WE", "WITHHOLDING TAX-EXPANDED"),
    ("WF", "WITHHOLDING TAX-FINAL"),
    ("WG", "WITHHOLDING TAX-VAT AND OTHER PERCENTAGE TAXES"),
    ("WO", "WITHHOLDING TAX-OTHERS"),
    ("WR", "WITHHOLDING TAX-FRINGE BENEFITS"),
    ("WW", "WITHHOLDING TAX-PERCENTAGE TAX ON WINNING AND PRIZES"),
    ("WV", "WITHHOLDING TAX-VALUE ADDED TAX (VAT)"),
    ("WP", "WITHHOLDING TAX-PERCENTAGE TAX"),
    ("XA", "EXCISE TAX-ALCOHOL"),
    ("XB", "EXCISE TAX-SWEETENED BEVERAGES"),
    ("XF", "EXCISE TAX-TOBACCO INSPECTION AND MONITORING FESS"),
    ("XG", "EXCISE TAX-AUTOMOBILES AND NON-ESSENTIALS"),
    ("XM", "EXCISE TAX-MINERALS"),
    ("XP", "EXCISE TAX-PETROLEUM"),
    ("XS", "EXCISE TAX-SPECIFIC"),
    ("XT", "EXCISE TAX-TOBACCO"),
    ("XV", "EXCISE TAX-AD VALOREM"),
    (
        "XC",
        "EXCISE TAX-PERFORMANCE OF SERVICES ON INVASIVE COSMETICS PROCEDURES",
    ),
    ("MA", "MICRO TAXPAYER ABATEMENT"),
];

/// Item 6 ATCs in `xml/atcCodes.xml` order (`AtcCode<n>` is n = index + 1).
pub const FORM_0605_ATCS: &[(&str, &str)] = &[
    ("FP010", "FINES AND PEN - ON TAX ON INCOME"),
    ("FP020", "FINES AND PEN - ON TAX ON TRANSFERS OF PROPERTY"),
    ("FP030", "FINES AND PEN - ON VALUE-ADDED TAX"),
    ("FP040", "FINES AND PEN - ON OTHER PERCENTAGE TAXES"),
    ("FP042", "FINES AND PEN - ON STOCK TRANS (IPO)"),
    ("FP050", "FINES AND PEN - ON EXCISE TAXES"),
    ("FP051", "FINES AND PEN - ON EXCISE SPECIFIC"),
    ("FP060", "FINES AND PEN - ON DOCUMENTARY STAMP TAXES"),
    ("FP070", "FINES AND PEN - ON MISCELLANEOUS TAXES"),
    ("FP071", "FINES AND PEN - ON ENERGY TAX"),
    ("FP090", "OTHERS FINES AND PENALTIES"),
    ("FP100", "FINES AND PEN - CAPITAL GAINS"),
    ("FP110", "FINES AND PEN - ON COMPENSATION"),
    ("FP120", "FINES AND PEN - ON FINAL"),
    ("FP130", "FINES AND PEN - ON EXPANDED"),
    ("FP140", "FINES AND PEN - GOVERNMENT MONEY"),
    ("FP141", "FINES AND PEN - WINNING AND PRIZES"),
    ("FP150", "FINES AND PEN - BANKS AND FINANCIAL INSTITUTION"),
    ("FP160", "FINES AND PEN - ESTATE TAX"),
    ("FP170", "FINES AND PEN - DONORS TAX"),
    (
        "FP180",
        "FINES AND PEN - W/T ON REAL PROP/M VEHICLES NOT SUBJ TO CG",
    ),
    ("FP190", "FINES AND PEN - REGISTRATION FEE"),
    ("FP930", "FINES AND PEN - INSPECTION FEES"),
    ("II011", "PURE COMPENSATION"),
    ("II012", "PURE BUSINESS"),
    ("II013", "MIXED INCOME"),
    ("MC010", "TAX AMNESTY ON INCOME (INDIVIDUAL)"),
    ("MC020", "TAX AMNESTY ON INCOME (CORPORATE)"),
    (
        "MC030",
        "COMP. PYMTS ON DELQNT. ACCOUNTS AND DISP. ASSESSMENTS",
    ),
    ("MC031", "DEFICIENCY TAX"),
    ("MC040", "INCOME FROM FORFEITED PROPERTIES"),
    ("MC050", "PROCEEDS FROM RESALE OF ESTATE TAKEN FOR TAXES"),
    ("MC060", "ENERGY TAX ON EXCESS ELECTRIC POWER CONSUMPTION"),
    ("MC090", "TIN CARD FEES"),
    ("MC180", "REGISTRATION FEE FOR VAT/NON-VAT TAXPAYERS"),
    ("MC190", "TRAVEL TAX"),
    ("MC200", "OTHER MISCELLANEOUS TAXES"),
    ("MC210", "MISCELLANEOUS TAXES - OTHER TAX REVENUE"),
    (
        "MC220",
        "ADVANCE PAYMENT OF VALUE ADDED TAX ON PRIVILEGE STORE",
    ),
    ("MC230", "ADVANCE PAYMENT OF INCOME TAX  ON PRIVILEGE STORE"),
    (
        "MC240",
        "ADVANCE PAYMENT OF PERCENTAGE TAX ON PRIVILEGE STORE",
    ),
    ("VM160", "VAT ON MANUFACTURING - SUGAR"),
    ("XA010", "DIST SPIR PROD FR SAP OF NIPA, ETC UND SEC 138(a)"),
    ("XA020", "DIST SPIRIT PROD IN A POT STILL"),
    (
        "XA031",
        "DIST SPRT O.T. SAP OF NIPA, ETC UND SEC. 141 (B)(1)",
    ),
    (
        "XA032",
        "DIST SPRT O.T. SAP OF NIPA, ETC UND SEC. 141 (B)(2)",
    ),
    (
        "XA033",
        "DIST SPRT O.T. SAP OF NIPA, ETC UND SEC. 141 (B)(3)",
    ),
    (
        "XA040",
        "MEDICINAL PREP FLAVORING EXTRACTS AND ALL OTHER PREP",
    ),
    (
        "XA051",
        "BEER, LGR BEER, ALE, PRTR FERMIN LIQ, ETC UND SEC 143 (A)",
    ),
    (
        "XA052",
        "BEER, LGR BEER, ALE, PRTR FERMIN LIQ, ETC UND SEC 143 (B)",
    ),
    (
        "XA053",
        "BEER, LGR BEER, ALE, PRTR FERMIN LIQ, ETC UND SEC 143 (C)",
    ),
    ("XA061", "SPARKLING WINES/CHAMPAGNE, ETC UND SEC 142 (A)(1)"),
    ("XA062", "SPARKLING WINES/CHAMPAGNE, ETC UND SEC 142 (A)(2)"),
    (
        "XA070",
        "STILL WINES CONTAINING 14% ALCOHOL OR LESS ALCOHOL",
    ),
    ("XA080", "STILL WINES CONTAINING OVER 14% ALCOHOL"),
    ("XA090", "FORTIFIED WINES"),
    ("XB010", "SWEETENED JUICE DRINKS"),
    ("XB020", "SWEETENED TEA"),
    ("XB030", "CARBONATED BEVERAGES"),
    ("XB040", "FLAVORED WATER"),
    ("XB050", "ENERGY AND SPORTS DRINKS"),
    (
        "XB060",
        "POWDERED DRINKS NOT CLASSIFIED AS MILK, JUICE, TEA AND COFFEE",
    ),
    ("XB070", "CEREAL AND GRAIN BEVERAGES"),
    (
        "XB080",
        "OTHER NON-ALCOHOLIC BEVERAGES THAT CONTAIN ADDED SUGAR",
    ),
    ("XB090", "USING PURELY HIGH FRUCTOSE CORN SYRUP"),
    (
        "XB100",
        "USING PURELY COCONUT SAP SUGAR AND PURELY STEVIOL GLYCOSIDES",
    ),
    (
        "XC010",
        "PERFORMANCE OF SERVICES ON INVASIVE COSMETIC PROCEDURES",
    ),
    ("XG020", "AUTOMOBILES (GASOLINE), ENGINE DISPL UP TO 1600CC"),
    (
        "XG030",
        "AUTOMOBILES (GASOLINE), ENGINE DISPL OF 1601 TO 2000CC",
    ),
    (
        "XG040",
        "AUTOMOBILES (GASOLINE), ENGINE DISPL OF 2001 TO 2700CC",
    ),
    ("XG050", "AUTOMOBILES (GASOLINE), ENG DISPL 2701 OR OVER"),
    ("XG060", "AUTOMOBILES (DIESEL), ENG DISPL UP TO 1800CC"),
    ("XG070", "AUTOMOBILES (DIESEL), ENG DISPL OF 1801 TO 2300CC"),
    ("XG080", "AUTOMOBILES (DIESEL), ENG DISPL OF 2301 TO 3000CC"),
    ("XG090", "AUTOMOBILES (DIESEL), ENG DISPL OF 3001CC OR OVER"),
    ("XG100", "JEWELRY"),
    ("XG110", "PERFUMES AND TOILET WATERS"),
    ("XG120", "YACHTS AND OTHER VESSELS INTENDED FOR PLEASURE"),
    ("XM010", "COAL AND COKE"),
    ("XM020", "NON-METALLIC MINERALS AND QUARRY RESOURCES"),
    ("XM030", "GOLD AND CHROMITE"),
    ("XM040", "COPPER AND OTHER METALLIC MINERALS"),
    ("XM050", "INDIGENOUS PETROLEUM"),
    (
        "XM051",
        "LOCALLY EXTRACTED NATURAL GAS AND LIQUIFIED NATURAL GAS",
    ),
    ("XP010", "LUBRICATING OIL"),
    ("XP020", "GREASE"),
    ("XP030", "PROCESSED GAS"),
    ("XP040", "WAXES AND PETROLATUM"),
    ("XP060", "PREMIUM GASOLINE - UNLEADED"),
    ("XP070", "PREMIUM GASOLINE - LEADED"),
    ("XP080", "REGULAR GASOLINE"),
    ("XP090", "NAPTHA"),
    ("XP100", "NAPTHA TO USED FOR PETRO-CHEMICAL"),
    ("XP110", "AVIATION GASOLINE"),
    ("XP120", "AVIATION TURBO JET FUEL"),
    ("XP130", "KEROSENE"),
    ("XP131", "KEROSENE USED AS AVIATION FUEL"),
    ("XP140", "DIESEL FUEL AND SIMILAR FUEL OILS"),
    ("XP150", "LPG USED FOR MOTIVE POWER"),
    ("XP160", "LPG"),
    ("XP170", "ASPHALTS"),
    ("XP180", "BUNKER/REFINERY FUEL/FEEDSTOCK"),
    (
        "XP190",
        "BASESTOCKS FOR LUBRICATING OIL SAND GREASES, HVD, ETC.",
    ),
    ("XT010", "SMOKING TOBACCO AND OTHER PARTIALLY MANUF TOBACCO"),
    ("XT020", "CHEWING TOBACCO"),
    ("XT030", "CIGARS"),
    ("XT040", "CIGARETTES PACKED BY HAND"),
    ("XT050", "CIGARETTES PACKED BY MACHINE, SEC 145 (C)(1)"),
    ("XT060", "CIGARETTES PACKED BY MACHINE, SEC 145 (C)(2)"),
    ("XT070", "CIGARETTES PACKED BY MACHINE, SEC 145 (C)(3)"),
    ("XT080", "TOBACCO INSPECT FEES FOR EACH THOUSAND CIGARS"),
    ("XT090", "TOBACCO INSPECT FEES FOR EACH THOUSAND CIGARETTES"),
    (
        "XT100",
        "TOBACCO INSPECT FEE PER KG OF LEAF TOBACCO AND OTHER",
    ),
    ("XT110", "PER KG OF SCRAPS AND OTHER MANUF TOBACCO PROD"),
    (
        "XT120",
        "ADDL IMPT BLENDING TOBACCO INSPECT AND MONITOR FEE",
    ),
    ("XT130", "CIGARETTES PACKED BY MACHINE, SEC. 145 (C)(4)"),
    ("DS010", "DOCUMENTARY STAMP TAX IN GENERAL"),
    ("IC080", "INCOME TAX ON INTERNATIONAL CARRIERS"),
    (
        "PT010",
        "PERSONS EXEMPT FROM VAT UNDER SEC. 109(BB) (SEC. 116)",
    ),
    (
        "PT040",
        "DOMESTIC CARRIERS AND KEEPERS OF GARAGES (SEC. 117)",
    ),
    ("PT041", "PERCENTAGE TAX ON INTERNATIONAL CARRIERS"),
    ("PT060", "FRANCHISES ON GAS AND WATER UTILITIES (SEC. 119)"),
    (
        "PT070",
        "FRANCHISES ON RADIO/TV BROADCASTING COMPANIES WHOSE ANNUAL GROSS RECEIPTS DO NOT EXCEED P10 M (SEC. 119)",
    ),
    (
        "PT090",
        "OVERSEAS DISPATCH, MESSAGE OR CONVERSATION ORIGINATING FROM THE PHILIPPINES (SEC. 120)",
    ),
    ("PT140", "COCKPITS (SEC. 125)"),
    (
        "PT150",
        "TAX ON AMUSEMENT PLACES, SUCH AS CABARETS, NIGHT AND DAY CLUBS, VIDEOKE BARS, KARAOKE BARS, KARAOKE TELEVISION, KARAOKE BOXES, MUSIC LOUNGES AND OTHER SIMILAR ESTABLISHMENTS (SEC. 125)",
    ),
    ("PT160", "BOXING EXHIBITION (SEC. 125)"),
    ("PT170", "PROFESSIONAL BASKETBALL GAMES (SEC. 125)"),
    ("PT180", "JAI-ALAI AND RACE TRACKS (SEC. 125)"),
    (
        "PT105",
        "ON INTEREST, COMMISSIONS AND DISCOUNTS FROM LENDING ACTIVITIES AS WELL AS INCOME FROM FINANCIAL LEASING, ON THE BASIS OF REMAINING MATURITIES OF INSTRUMENTS FROM WHICH SUCH RECEIPTS ARE DERIVED. MATURITY PERIOD IS FIVE (5) YEARS OR LESS",
    ),
    (
        "PT101",
        "ON INTEREST, COMMISSIONS AND DISCOUNTS FROM LENDING ACTIVITIES AS WELL AS INCOME FROM FINANCIAL LEASING, ON THE BASIS OF REMAINING MATURITIES OF INSTRUMENTS FROM WHICH SUCH RECEIPTS ARE DERIVED. MATURITY PERIOD IS MORE THAN FIVE (5) YEARS",
    ),
    (
        "PT102",
        "ON DIVIDENDS AND EQUITY SHARES AND NET INCOME OF SUBSIDIARIES",
    ),
    (
        "PT103",
        "ON ROYALTIES, RENTALS OF PROPERTY, REAL OR PERSONAL, PROFITS FROM EXCHANGE AND ALL OTHER GROSS INCOME",
    ),
    (
        "PT104",
        "ON NET TRADING GAINS WITHIN THE TAXABLE YEAR ON FOREIGN CURRENCY, DEBT SECURITIES, DERIVATIVES AND OTHER FINANCIAL INSTRUMENTS",
    ),
    (
        "PT113",
        "ON INTEREST, COMMISSIONS AND DISCOUNTS FROM LENDING ACTIVITIES AS WELL AS INCOME FROM FINANCIAL LEASING, ON THE BASIS OF REMAINING MATURITIES OF INSTRUMENTS FROM WHICH SUCH RECEIPTS ARE DERIVED. MATURITY PERIOD IS FIVE (5) YEARS OR LESS",
    ),
    (
        "PT114",
        "ON INTEREST, COMMISSIONS AND DISCOUNTS FROM LENDING ACTIVITIES AS WELL AS INCOME FROM FINANCIAL LEASING, ON THE BASIS OF REMAINING MATURITIES OF INSTRUMENTS FROM WHICH SUCH RECEIPTS ARE DERIVED. MATURITY PERIOD IS MORE THAN FIVE (5) YEARS",
    ),
    (
        "PT115",
        "FROM ALL OTHER ITEMS TREATED AS GROSS INCOME UNDER THE CODE",
    ),
    ("PT120", "LIFE INSURANCE PREMIUMS (SEC. 123)"),
    ("PT130", "INSURANCE AGENTS"),
    (
        "PT132",
        "OWNERS OF PROPERTY OBTAINING INSURANCE DIRECTLY WITH FOREIGN INSURANCE COMPANIES",
    ),
    ("EXB10", "EXCISE TAX ON EXPORT OF SWEETENED JUICE DRINKS"),
    (
        "WV110",
        "VAT ON LOCAL SALES OF REGISTERED BUSINESS ENTERPRISES (RBEs)",
    ),
    (
        "MC350",
        "ONE-TIME ABATEMENT OF TAXES AND/OR PENALTIES FOR MICRO TAXPAYERS (INDIVIDUAL)",
    ),
    (
        "MC351",
        "ONE-TIME ABATEMENT OF TAXES AND/OR PENALTIES FOR MICRO TAXPAYERS (CORPORATION)",
    ),
];

/// The official `validate()` alert for an Item 4 (`item == 4`) or Item 7
/// date typed as `MM/DD/YYYY`, following the page's branches in order: the
/// early format / day / leap-year / month-length checks, then the later
/// month, day, date and year checks. `None` when the text is a valid date,
/// or blank Item 7 (reported by `official_errors`). A blank Item 4 gets
/// "Invalid date entry on item 4." like the page, whose `day < 1` check
/// coerces an empty day to 0. The official page has one numeric box per part; a
/// part that is not digits gets the format alert.
pub fn form_0605_typed_date_alert(item: u8, text: &str) -> Option<&'static str> {
    let item7 = item == 7;
    let text = text.trim();
    if text.is_empty() {
        return (!item7).then_some("Invalid date entry on item 4.");
    }
    let mut parts = text.split('/');
    let month = parts.next().unwrap_or("").trim();
    let day = parts.next().unwrap_or("").trim();
    let year = parts.next().unwrap_or("").trim();
    let format = if item7 {
        "Please enter a valid day/month on item 7. Format should be MM/DD/YYYY."
    } else {
        "Please enter a valid day/month on item 4. Format should be MM/DD/YYYY."
    };
    if parts.next().is_some()
        || [month, day, year]
            .iter()
            .any(|p| p.len() > 4 || !p.chars().all(|c| c.is_ascii_digit()))
    {
        return Some(format);
    }
    // JavaScript coerces "" to 0 in these comparisons.
    let num = |p: &str| p.parse::<u32>().unwrap_or(0);
    let (m, d, y) = (num(month), num(day), num(year));
    let invalid_entry = if item7 {
        "Invalid date entry on item 7."
    } else {
        "Invalid date entry on item 4."
    };
    if item7 && (month.is_empty() || day.is_empty() || year.is_empty()) {
        return Some("Please enter a valid Return Period on Item 7.");
    }
    if month.len() == 1 || day.len() == 1 {
        return Some(format);
    }
    if d < 1 {
        return Some(invalid_entry);
    }
    // new Date(year, 1, 29).getMonth() == 1; years 0-99 map to 1900-1999.
    let js_year = if y < 100 { 1900 + y } else { y };
    let gregorian_leap =
        |y: u32| y.is_multiple_of(4) && (!y.is_multiple_of(100) || y.is_multiple_of(400));
    let is_leap = gregorian_leap(js_year);
    if !is_leap && m == 2 && d == 29 {
        return Some("Filing year is not a leap year.");
    }
    if m == 2 && d > 29 {
        return Some(invalid_entry);
    }
    const MONTH31: [&str; 7] = ["01", "03", "05", "07", "08", "10", "12"];
    const MONTH30: [&str; 4] = ["04", "06", "09", "11"];
    if (MONTH31.contains(&month) && d > 31) || (MONTH30.contains(&month) && d > 30) {
        return Some(invalid_entry);
    }
    let month31 = matches!(m, 1 | 3 | 5 | 7 | 8 | 10 | 12);
    let month30 = matches!(m, 4 | 6 | 9 | 11);
    let feb_max = if gregorian_leap(y) { 29 } else { 28 };
    if item7 {
        if month.is_empty() || !(1..=12).contains(&m) {
            return Some("Please enter a valid month on item 7.");
        }
        if day.is_empty() || (month31 && !(1..=31).contains(&d)) {
            return Some("Please enter a valid day on item 7.");
        }
        if month30 && !(1..=30).contains(&d) {
            return Some("Please enter a valid date on item 7.");
        }
        if m == 2 {
            if year.is_empty() {
                return Some("Please enter a valid year on item 7.");
            }
            if !(1..=feb_max).contains(&d) {
                return Some("Please enter a valid date on item 7.");
            }
        }
        if year.is_empty() {
            return Some("Please enter a valid year on item 7.");
        }
        if !(1904..=3000).contains(&y) {
            return Some("Please enter a valid date on item 7.");
        }
    } else {
        if month.is_empty() || !(1..=12).contains(&m) {
            return Some("Please enter valid month on item 4.");
        }
        if month31 {
            if day.is_empty() {
                return Some("Please enter a valid day on item 4.");
            }
            if !(1..=31).contains(&d) {
                return Some("Please enter a valid date on item 4.");
            }
        }
        if m == 2 {
            if day.is_empty() {
                return Some("Please enter a valid day on item 4.");
            }
            if !(1..=feb_max).contains(&d) {
                return Some("Please enter a valid date on item 4.");
            }
        }
        if year.is_empty() {
            return Some("Please enter valid year on item 4.");
        }
        if !(1904..=3000).contains(&y) {
            return Some("Please enter a valid date on item 4.");
        }
    }
    // Branches the page leaves open (e.g. 04/31 on Item 4) still cannot be
    // stored as a calendar date.
    if Form0605Date::new(y as u16, m as u8, d as u8).is_err() {
        return Some(invalid_entry);
    }
    None
}

/// The official tax type at a 1-based `TaxTypeCode<n>` index.
pub fn form_0605_tax_type(code: &str) -> Option<Form0605IndexedCode> {
    FORM_0605_TAX_TYPES
        .iter()
        .position(|(candidate, _)| *candidate == code)
        .map(|index| Form0605IndexedCode::official(code, index as u16 + 1))
}

/// The official ATC at a 1-based `AtcCode<n>` index.
pub fn form_0605_atc(code: &str) -> Option<Form0605IndexedCode> {
    FORM_0605_ATCS
        .iter()
        .position(|(candidate, _)| *candidate == code)
        .map(|index| Form0605IndexedCode::official(code, index as u16 + 1))
}

/// `ATCList()` hides the PT ATCs unless the tax type is PT, and only PT ATCs
/// when it is.
pub fn form_0605_atc_offered_for(atc: &str, tax_type: &str) -> bool {
    (tax_type == "PT") == atc.starts_with("PT")
}

/// `round(this, 2)` / `formatCurrency`: the value the official field holds.
fn cents(value: f64) -> f64 {
    parse_official_amount(&official_amount(value)).unwrap_or(0.0)
}

fn capital(value: &str) -> String {
    value.trim().to_uppercase()
}

fn digits_only(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit())
}

/// The table entry an imported or picked code must match exactly.
fn official_pair(selection: &Form0605IndexedCode, table: &[(&str, &str)]) -> bool {
    table
        .get(usize::from(selection.xml_index()).wrapping_sub(1))
        .is_some_and(|(code, _)| *code == selection.code())
}

impl Form0605Draft {
    /// `computePenalties` / `computeOfTax`, on the values `round(this,2)`
    /// leaves in the fields.
    pub(super) fn official_recompute(&mut self) {
        if self.filing_basis == Form0605FilingBasis::Calendar {
            // dateyear(): calendar filers end in December.
            self.year_end_month = 12;
        }
        self.item_19_basic_tax_or_payment = cents(self.item_19_basic_tax_or_payment);
        // The penalty items file their override, else the computed value.
        Self::penalty_declaration().apply(self);
        self.item_20a_surcharge = cents(self.item_20a_surcharge);
        self.item_20b_interest = cents(self.item_20b_interest);
        self.item_20c_compromise = cents(self.item_20c_compromise);
        self.item_20d_total_penalties =
            cents(self.item_20a_surcharge + self.item_20b_interest + self.item_20c_compromise);
        self.item_21_total_amount_payable =
            cents(self.item_19_basic_tax_or_payment + self.item_20d_total_penalties);
        // disabletxtOthers() / disableNumInstallment() / disableDefTax().
        if self.manner_of_payment != Some(Form0605MannerOfPayment::Others) {
            self.other_manner_description.clear();
        }
        if self.type_of_payment != Some(Form0605TypeOfPayment::Installment) {
            self.number_of_installments = None;
        }
        if !matches!(
            self.manner_of_payment,
            Some(
                Form0605MannerOfPayment::PreliminaryOrFinalAssessmentOrDeficiencyTax
                    | Form0605MannerOfPayment::AccountsReceivableOrDelinquentAccount
            )
        ) {
            self.approval_selection = Form0605ApprovalSelection::None;
        }
    }

    /// `validate()` and `initialValidateBeforeSave()` in order with their
    /// alert texts, plus the limits the page enforces while typing.
    pub(super) fn official_errors(&self) -> Vec<(String, String)> {
        let mut errors = Vec::new();
        let mut err = |field: &str, message: &str| {
            errors.push((field.to_string(), message.to_string()));
        };

        // Item 2
        if !(1904..=3000).contains(&self.taxable_year) {
            err("taxable_year", "Please enter a valid year on Item 2.");
        }
        if !(1..=12).contains(&self.year_end_month) {
            err("year_end_month", "Please select a valid month on Item 2.");
        } else if self.filing_basis == Form0605FilingBasis::Fiscal && self.year_end_month == 12 {
            err(
                "year_end_month",
                "You have entered invalid month for Fiscal Year",
            );
        }
        if self.quarter > 4 {
            err("quarter", "Item 3 quarter must be from 1 to 4.");
        }
        // Item 4: validate() rule 016 (`txtDueDateDay.value < 1`) also
        // rejects a blank due date, since "" coerces to 0.
        match self.due_date {
            None => err("due_date", "Invalid date entry on item 4."),
            Some(due) if due.validate().is_err() => {
                err("due_date", "Invalid date entry on item 4.")
            }
            Some(due) if !(1904..=3000).contains(&due.year) => {
                err("due_date", "Please enter a valid date on item 4.")
            }
            Some(_) => {}
        }
        if self.number_of_sheets > 99 {
            err("number_of_sheets", "Item 5 holds at most two digits.");
        }
        // Item 6
        let atc_ok = self
            .atc
            .as_ref()
            .is_some_and(|atc| official_pair(atc, FORM_0605_ATCS));
        if !atc_ok {
            err("atc", "Please enter a valid ATC on Item 6.");
        }
        // Item 7
        match self.return_period {
            None => err(
                "return_period",
                "Please enter a valid Return Period on Item 7.",
            ),
            Some(period) if period.validate().is_err() => {
                err("return_period", "Invalid date entry on item 7.")
            }
            Some(period) if !(1904..=3000).contains(&period.year) => {
                err("return_period", "Please enter a valid date on item 7.")
            }
            Some(_) => {}
        }
        // Item 9
        let digits: String = self.tin.chars().filter(char::is_ascii_digit).collect();
        if digits.len() < 12
            || digits.len() > 14
            || !self.tin.chars().all(|ch| ch.is_ascii_digit() || ch == '-')
        {
            err("tin", "Please enter a valid TIN number on Item 9.");
        } else if !crate::validation::relaxed_dev_mode()
            && crate::validation::official_tin_check_code(&digits[..9]) != 0
        {
            err("tin", crate::validation::OFFICIAL_INVALID_TIN_MESSAGE);
        }
        // Item 10
        let rdo = self.rdo_code.trim();
        if rdo.is_empty() || rdo == "000" || !crate::validation::rdo_code_is_official_option(rdo) {
            err("rdo_code", "Please enter a valid RDO Code on Item 10.");
        }
        if self.line_of_business.trim().is_empty() {
            err(
                "line_of_business",
                "Please enter a valid Line of Business/Occupation on Item 12.",
            );
        }
        let name = self.taxpayer_name.trim();
        if name.is_empty() || name.chars().count() > 75 {
            err(
                "taxpayer_name",
                "Please enter a valid Taxpayer Name on Item 13.",
            );
        }
        if !digits_only(self.contact_number.trim()) {
            err(
                "contact_number",
                "Please enter a valid Taxpayer Telephone Number on Item 14.",
            );
        }
        if self.registered_address.trim().is_empty() {
            err(
                "registered_address",
                "Please enter a valid Taxpayer Registered Address on Item 15.",
            );
        }
        if !digits_only(self.zip_code.trim()) {
            err(
                "zip_code",
                "Please enter a valid Taxpayer Zip Code on Item 16.",
            );
        }
        if let Some(period) = self.return_period
            && (period.year > self.taxable_year
                || (period.year == self.taxable_year && period.month > self.year_end_month))
        {
            err(
                "return_period",
                "The return period date should not be later than the year ended date.",
            );
        }
        // Item 8
        let tax_type_ok = self
            .tax_type
            .as_ref()
            .is_some_and(|tax_type| official_pair(tax_type, FORM_0605_TAX_TYPES));
        if !tax_type_ok {
            err("tax_type", "Please select Tax Type Code on Item 8.");
        } else if let (Some(atc), Some(tax_type)) = (&self.atc, &self.tax_type)
            && atc_ok
            && !form_0605_atc_offered_for(atc.code(), tax_type.code())
        {
            err("atc", "Please enter a valid ATC on Item 6.");
        }
        // Item 17
        match self.manner_of_payment {
            None => err(
                "manner_of_payment",
                "Please select Manner of Payment on item 17.",
            ),
            Some(
                Form0605MannerOfPayment::PreliminaryOrFinalAssessmentOrDeficiencyTax
                | Form0605MannerOfPayment::AccountsReceivableOrDelinquentAccount,
            ) if self.approval_selection == Form0605ApprovalSelection::None => err(
                "approval_selection",
                "Since you have selected Preliminary/Final Assess/Deficiency Tax on\nitem 17, you must choose either:Pre-approved or Not approved byInvestigating Office.",
            ),
            Some(Form0605MannerOfPayment::Others)
                if self.other_manner_description.trim().is_empty() =>
            {
                err(
                    "other_manner_description",
                    "Specify the other manner of payment on Item 17.",
                )
            }
            _ => {}
        }
        if self.other_manner_description.chars().count() > 60 {
            err(
                "other_manner_description",
                "Item 17 Others holds at most 60 characters.",
            );
        }
        // Item 18
        if self.type_of_payment == Some(Form0605TypeOfPayment::Installment)
            && !self
                .number_of_installments
                .is_some_and(|count| (1..=20).contains(&count))
        {
            err(
                "number_of_installments",
                "Please re-enter No. of Installment. Allowed values from 1 to 20 only.",
            );
        }
        if self.type_of_payment.is_none() {
            err(
                "type_of_payment",
                "Please select Type of Payment on item 18.",
            );
        }
        // Item 19
        if self.item_19_basic_tax_or_payment <= 0.0 {
            err(
                "item_19_basic_tax_or_payment",
                "Please enter valid value (greater than 0) for item 19 under Computation of Tax.",
            );
        }
        for (field, value) in [
            (
                "item_19_basic_tax_or_payment",
                self.item_19_basic_tax_or_payment,
            ),
            ("item_20a_surcharge", self.item_20a_surcharge),
            ("item_20b_interest", self.item_20b_interest),
            ("item_20c_compromise", self.item_20c_compromise),
        ] {
            if value < 0.0 || !has_cent_precision(value) {
                err(field, "Enter a non-negative amount in pesos and centavos.");
            }
        }
        // Part III payment details (printed, not submitted).
        let payments = &self.payment_details;
        for (field, value) in [
            (
                "payment_23_cash_or_bank_debit_memo.amount",
                payments.cash_or_bank_debit_memo_amount,
            ),
            ("payment_24_check.amount", payments.check.amount),
            (
                "payment_25_tax_debit_memo.amount",
                payments.tax_debit_memo.amount,
            ),
            ("payment_26_others.amount", payments.others.amount),
        ] {
            if value.is_some_and(|amount| !amount.is_finite() || amount < 0.0) {
                err(
                    field,
                    "Payment amount must be a finite, non-negative number",
                );
            }
        }
        for (field, value) in [
            ("payment_24_check.date", payments.check.date.as_str()),
            (
                "payment_25_tax_debit_memo.date",
                payments.tax_debit_memo.date.as_str(),
            ),
            ("payment_26_others.date", payments.others.date.as_str()),
        ] {
            if !value.trim().is_empty()
                && chrono::NaiveDate::parse_from_str(value.trim(), "%m/%d/%Y").is_err()
            {
                err(
                    field,
                    "Payment date must use MM/DD/YYYY and be a real calendar date",
                );
            }
        }
        let email = self.email.trim();
        if email.is_empty() || !email.contains('@') || email.contains(char::is_whitespace) {
            err(
                "email",
                "Enter the email address BIR sends the filing confirmation to.",
            );
        }
        if self.month == 0 {
            err("month", "Open-ended persistence slot must be at least 1");
        }

        let mut expected = self.clone();
        expected.official_recompute();
        let penalties = Self::penalty_declaration();
        if expected.item_21_total_amount_payable != self.item_21_total_amount_payable
            || penalties.filed(&expected) != penalties.filed(self)
            || expected.item_20d_total_penalties != self.item_20d_total_penalties
            || expected.item_19_basic_tax_or_payment != self.item_19_basic_tax_or_payment
        {
            err(
                "item_21_total_amount_payable",
                "Totals are out of date. Recompute the return.",
            );
        }
        errors
    }

    /// The official field values `saveXMLsubmit` reads, keyed by element id,
    /// including the popup radios (`AtcCode<n>`, `TaxTypeCode<n>`).
    pub fn to_official_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = BTreeMap::new();
        let mut put = |key: &str, value: String| {
            fields.insert(key.to_string(), value);
        };
        let flag = |on: bool| on.to_string();
        let date_parts = |date: Option<Form0605Date>| match date {
            Some(d) => (
                format!("{:02}", d.month),
                format!("{:02}", d.day),
                format!("{:04}", d.year),
            ),
            None => Default::default(),
        };

        let calendar = self.filing_basis == Form0605FilingBasis::Calendar;
        put("frm0605:itemFiscalStartMonth:_1", flag(calendar));
        put("frm0605:itemFiscalStartMonth:_2", flag(!calendar));
        for quarter in 1..=4u8 {
            put(
                &format!("itemQuarter_{quarter}"),
                flag(self.quarter == quarter),
            );
        }
        let (m, d, y) = date_parts(self.due_date);
        put("frm0605:txtDueDateMonth", m);
        put("frm0605:txtDueDateDay", d);
        put("frm0605:txtDueDateYear", y);
        put("frm0605:txtNoOfSheets", self.number_of_sheets.to_string());
        put(
            "txtATCCode",
            self.atc
                .as_ref()
                .map(|a| a.code().to_string())
                .unwrap_or_default(),
        );
        put(
            "frm0605:itemYearEndMonth",
            format!("{:02}", self.year_end_month),
        );
        put("frm0605:txtYearEnded", self.taxable_year.to_string());
        let (m, d, y) = date_parts(self.return_period);
        put("frm0605:txtReturnPeriodMonth", m);
        put("frm0605:txtReturnPeriodDay", d);
        put("frm0605:txtReturnPeriodYear", y);
        put(
            "txtTaxTypeCode",
            self.tax_type
                .as_ref()
                .map(|t| t.code().to_string())
                .unwrap_or_default(),
        );
        let digits: String = self.tin.chars().filter(char::is_ascii_digit).collect();
        let part = |range: std::ops::Range<usize>| digits.get(range).unwrap_or("").to_string();
        put("frm0605:txtTIN1", part(0..3));
        put("frm0605:txtTIN2", part(3..6));
        put("frm0605:txtTIN3", part(6..9));
        put(
            "frm0605:txtBranchCode",
            digits.get(9..).unwrap_or("").to_string(),
        );
        put("frm0605:txtRDOCode", self.rdo_code.trim().to_string());
        let individual = self.classification == Form0605TaxpayerClassification::Individual;
        put("frm0605:txtClassification:_1", flag(individual));
        put("frm0605:txtClassification:_2", flag(!individual));
        // capital() uppercases every text control except txtEmail.
        put("frm0605:txtLineBus", capital(&self.line_of_business));
        put("frm0605:txtTaxPayerName", capital(&self.taxpayer_name));
        put("frm0605:txtTelNum", self.contact_number.trim().to_string());
        put("frm0605:txtAddress", capital(&self.registered_address));
        put("frm0605:txtZipCode", self.zip_code.trim().to_string());
        let mode = self.type_of_payment;
        put(
            "frm0605:itemModeOfPayment:_1",
            flag(mode == Some(Form0605TypeOfPayment::Installment)),
        );
        put(
            "frm0605:txtNumOfInstallment",
            self.number_of_installments
                .filter(|_| mode == Some(Form0605TypeOfPayment::Installment))
                .map(|count| count.to_string())
                .unwrap_or_default(),
        );
        put(
            "frm0605:itemModeOfPayment:_2",
            flag(mode == Some(Form0605TypeOfPayment::PartialPayment)),
        );
        put(
            "frm0605:itemModeOfPayment:_3",
            flag(mode == Some(Form0605TypeOfPayment::FullPayment)),
        );
        use Form0605MannerOfPayment as M;
        for (key, manner) in [
            // Official BIR-Form0605.hta control order (see form_0605_xml.rs).
            ("frm0605:itemMannerOfPayment:_1", M::SelfAssessment),
            ("frm0605:itemMannerOfPayment:_2", M::Penalties),
            (
                "frm0605:itemMannerOfPayment:_3",
                M::TaxDepositOrAdvancePayment,
            ),
            (
                "frm0605:itemMannerOfPayment:_4",
                M::IncomeTaxSecondInstallmentIndividual,
            ),
            ("frm0605:itemMannerOfPayment:_5", M::Others),
            (
                "frm0605:itemMannerOfPaymentB:_1",
                M::PreliminaryOrFinalAssessmentOrDeficiencyTax,
            ),
            (
                "frm0605:itemMannerOfPaymentB:_2",
                M::AccountsReceivableOrDelinquentAccount,
            ),
        ] {
            put(key, flag(self.manner_of_payment == Some(manner)));
        }
        put(
            "frm0605:txtOthersName",
            capital(&self.other_manner_description),
        );
        put(
            "frm0605:txtTax19",
            official_amount(self.item_19_basic_tax_or_payment),
        );
        put(
            "frm0605:txtTax20A",
            official_amount(self.item_20a_surcharge),
        );
        put("frm0605:txtTax20B", official_amount(self.item_20b_interest));
        put(
            "frm0605:txtTax20C",
            official_amount(self.item_20c_compromise),
        );
        put(
            "frm0605:txtTax20D",
            official_amount(self.item_20d_total_penalties),
        );
        put(
            "frm0605:txtTax21",
            official_amount(self.item_21_total_amount_payable),
        );
        put(
            "frm0605:itemApprovedYN:_1",
            flag(self.approval_selection == Form0605ApprovalSelection::XmlOption1),
        );
        put(
            "frm0605:itemApprovedYN:_2",
            flag(self.approval_selection == Form0605ApprovalSelection::XmlOption2),
        );
        // ATCList() runs once a tax type is chosen; TaxTypeCodeList() at load.
        if self.tax_type.is_some() {
            let picked = self.atc.as_ref().map(Form0605IndexedCode::xml_index);
            for index in 1..=FORM_0605_ATCS.len() as u16 {
                put(&format!("AtcCode{index}"), flag(picked == Some(index)));
            }
        }
        let picked = self.tax_type.as_ref().map(Form0605IndexedCode::xml_index);
        for index in 1..=FORM_0605_TAX_TYPES.len() as u16 {
            put(&format!("TaxTypeCode{index}"), flag(picked == Some(index)));
        }
        put("txtEmail", self.email.trim().to_string());
        fields
    }

    /// The exact official submit plaintext.
    pub fn to_official_xml_payload(&self) -> Result<String, Vec<(String, String)>> {
        self.official_payload()
    }
}

fn is_popup_radio(key: &str) -> bool {
    ["AtcCode", "TaxTypeCode"].iter().any(|prefix| {
        key.strip_prefix(prefix)
            .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
    })
}

impl Form0605Draft {
    /// Items 20A-20C on the shared penalty model ([`super::penalty_items`]).
    pub fn penalty_declaration() -> PenaltyItems<Self> {
        super::penalty_items::queueable_penalty_items!(
            Self,
            schedule: StandardFiling,
            items: [
                item_20a_surcharge: "20A Surcharge",
                item_20b_interest: "20B Interest",
                item_20c_compromise: "20C Compromise",
            ],
            controls: [
                "frm0605:txtTax20A",
                "frm0605:txtTax20B",
                "frm0605:txtTax20C",
            ],
            state: penalty,
            basic_tax_due: "Item 19", |d| d.item_19_basic_tax_or_payment,
        )
    }
}

impl QueueableForm for Form0605Draft {
    fn print_field_map(&self) -> BTreeMap<String, String> {
        self.to_bir_field_map()
    }
    fn penalty_items() -> Vec<PenaltyItems<Self>> {
        vec![Self::penalty_declaration()]
    }
    const FORM_CODE: &'static str = "0605";
    const FORM_TYPE: &'static str = FORM_0605_FORM_TYPE;
    const LAYOUT_ID: &'static str = FORM_0605_LAYOUT_ID;

    /// Item 21 Total Amount Payable.
    fn amount_payable(&self) -> Option<f64> {
        Some(self.item_21_total_amount_payable)
    }

    fn lifecycle(&self) -> &SubmissionLifecycle {
        &self.lifecycle
    }
    fn lifecycle_mut(&mut self) -> &mut SubmissionLifecycle {
        &mut self.lifecycle
    }
    fn draft_id(&self) -> Option<i64> {
        self.id
    }
    fn set_draft_id(&mut self, id: Option<i64>) {
        self.id = id;
    }
    fn tin(&self) -> &str {
        &self.tin
    }
    fn taxable_year(&self) -> u16 {
        self.taxable_year
    }
    /// The dashboard's open-ended slot, as the pre-generic editor stored it
    /// (`period_key` `O<month>`).
    fn filing_period(&self) -> FilingPeriod {
        FilingPeriod::OpenEnded(u32::from(self.month))
    }
    /// `createXMLFileName`: Item 7 as `MMDDYYYY` plus the `HHMMSS` the file
    /// was made at (`getHHMMSS`). The draft's creation time (Philippine
    /// time) stands in for that clock so the code is stable once queued.
    fn period_code(&self) -> String {
        let period = self
            .return_period
            .map(|d| format!("{:02}{:02}{:04}", d.month, d.day, d.year))
            .unwrap_or_default();
        let clock = chrono::DateTime::parse_from_rfc3339(&self.lifecycle.created_at)
            .ok()
            .and_then(|at| chrono::FixedOffset::east_opt(8 * 3600).map(|ph| at.with_timezone(&ph)))
            .map(|at| at.format("%H%M%S").to_string())
            .unwrap_or_else(|| "000000".to_string());
        format!("{period}{clock}")
    }
    /// The filename carries Item 7 and a clock time, not the dashboard slot
    /// that keys the stored return, so a receipt cannot name its row.
    fn parse_period_code(_code: &str) -> Option<(u16, FilingPeriod)> {
        None
    }
    fn submission_email(&self) -> &str {
        self.email.trim()
    }
    fn compute(&mut self) {
        self.recompute();
    }
    fn validate(&self) -> Vec<(String, String)> {
        <Self as FormValidator>::validate(self)
    }
    fn field_map(&self) -> BTreeMap<String, String> {
        self.to_official_field_map()
    }

    /// The layout replayed over the field map, with the popup radios written
    /// where the page holds them (just before `txtFinalFlag`).
    fn official_payload(&self) -> Result<String, Vec<(String, String)>> {
        let errors = <Self as QueueableForm>::validate(self);
        if !errors.is_empty() {
            return Err(errors);
        }
        let xml_error = |error: crate::official_xml::OfficialXmlError| {
            vec![("xml".to_string(), error.to_string())]
        };
        let layout = crate::official_xml::layout(Self::LAYOUT_ID).map_err(xml_error)?;
        let mut fields = self.field_map();
        let radios: Vec<(String, String)> = ["AtcCode", "TaxTypeCode"]
            .iter()
            .flat_map(|prefix| {
                let count = if *prefix == "AtcCode" {
                    FORM_0605_ATCS.len()
                } else {
                    FORM_0605_TAX_TYPES.len()
                };
                (1..=count).map(move |index| format!("{prefix}{index}"))
            })
            .filter_map(|key| fields.get(&key).map(|value| (key.clone(), value.clone())))
            .collect();
        fields.retain(|key, _| !is_popup_radio(key));
        let mut text = crate::official_xml::write(layout, &fields).map_err(xml_error)?;
        // Every control on this page is followed by the same `tab` text.
        let tab = &layout.lead;
        let inserted: String = radios
            .iter()
            .map(|(key, value)| format!("<div>{key}={value}{key}=</div>{tab}"))
            .collect();
        let at = text.find("<div>txtFinalFlag=").ok_or_else(|| {
            vec![(
                "xml".to_string(),
                "txtFinalFlag is missing from the layout".to_string(),
            )]
        })?;
        text.insert_str(at, &inserted);
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filing_queue::QueueAuthSource;
    use crate::forms::FilingStatus;

    fn profile() -> crate::profile::TaxpayerProfile {
        serde_json::from_value(serde_json::json!({
            "id": null,
            "full_name": "Sample Dummy Taxpayer",
            "tin": {"segment1": "123", "segment2": "456", "segment3": "788", "branch": "00000"},
            "rdo_code": "039",
            "line_of_business": "Sample Consulting Services",
            "registered_address": "123 Sample Street, Quezon City",
            "zip_code": "1100",
            "phone": "09170000000",
            "email": "sample.taxpayer@example.com",
            "default_form_type": "0605",
            "taxpayer_type": "Individual"
        }))
        .expect("dummy profile")
    }

    pub(crate) fn sample() -> Form0605Draft {
        let mut draft = Form0605Draft::new_from_profile(&profile(), 2025, 1);
        draft.lifecycle.created_at = "2025-12-01T01:02:03+00:00".into();
        draft.return_period = Form0605Date::new(2025, 12, 31).ok();
        draft.due_date = Form0605Date::new(2026, 1, 25).ok();
        draft.tax_type = form_0605_tax_type("IT");
        draft.atc = form_0605_atc("II011");
        draft.manner_of_payment = Some(Form0605MannerOfPayment::SelfAssessment);
        draft.type_of_payment = Some(Form0605TypeOfPayment::FullPayment);
        draft.item_19_basic_tax_or_payment = 12_345.675;
        draft.item_20a_surcharge = 25.5;
        draft.keep_entered_penalties();
        draft.recompute();
        draft
    }

    fn messages(draft: &Form0605Draft) -> Vec<String> {
        <Form0605Draft as QueueableForm>::validate(draft)
            .into_iter()
            .map(|(_, message)| message)
            .collect()
    }

    #[test]
    fn code_tables_match_the_reviewed_indexes() {
        assert_eq!(form_0605_atc("FP010").unwrap().xml_index(), 1);
        assert_eq!(form_0605_atc("II011").unwrap().xml_index(), 24);
        assert_eq!(form_0605_tax_type("DO").unwrap().xml_index(), 4);
        assert_eq!(form_0605_tax_type("IT").unwrap().xml_index(), 9);
        assert_eq!(FORM_0605_ATCS.len(), 144);
        assert_eq!(FORM_0605_TAX_TYPES.len(), 40);
        assert!(form_0605_atc_offered_for("II011", "IT"));
        assert!(!form_0605_atc_offered_for("PT010", "IT"));
    }

    #[test]
    fn blank_item_4_due_date_gets_the_official_rule_016_alert() {
        let mut draft = sample();
        draft.due_date = None;
        assert!(
            <Form0605Draft as QueueableForm>::validate(&draft)
                .iter()
                .any(|(field, message)| field == "due_date"
                    && message == "Invalid date entry on item 4.")
        );
    }

    #[test]
    fn compute_chain_matches_the_official_page() {
        let draft = sample();
        assert_eq!(draft.item_19_basic_tax_or_payment, 12_345.68);
        assert_eq!(draft.item_20d_total_penalties, 25.5);
        assert_eq!(draft.item_21_total_amount_payable, 12_371.18);
        assert!(messages(&draft).is_empty(), "{:?}", messages(&draft));
    }

    #[test]
    fn field_map_and_filename_follow_the_official_page() {
        let draft = sample();
        let fields = draft.to_official_field_map();
        assert_eq!(fields["frm0605:txtTax21"], "12,371.18");
        assert_eq!(fields["frm0605:txtTaxPayerName"], "SAMPLE DUMMY TAXPAYER");
        assert_eq!(fields["AtcCode24"], "true");
        assert_eq!(fields["TaxTypeCode9"], "true");
        assert_eq!(fields["frm0605:itemYearEndMonth"], "12");
        assert_eq!(draft.period_code(), "12312025090203");
        assert_eq!(
            draft.submission_filename(),
            "12345678800000-0605-12312025090203#sample.taxpayer@example.com#.xml"
        );
        let payload = draft.to_official_xml_payload().unwrap();
        let atc = payload.find("<div>AtcCode1=").unwrap();
        let tax = payload.find("<div>TaxTypeCode1=").unwrap();
        let approved = payload.find("<div>frm0605:itemApprovedYN:_2=").unwrap();
        let final_flag = payload.find("<div>txtFinalFlag=").unwrap();
        assert!(approved < atc && atc < tax && tax < final_flag);
    }

    #[test]
    fn official_negative_cases_are_rejected_with_their_alerts() {
        let check = |mutate: &dyn Fn(&mut Form0605Draft), expected: &str| {
            let mut draft = sample();
            mutate(&mut draft);
            let found = messages(&draft);
            assert!(
                found.iter().any(|m| m == expected),
                "{expected:?} not in {found:?}"
            );
        };
        check(
            &|d| d.taxable_year = 1800,
            "Please enter a valid year on Item 2.",
        );
        check(
            &|d| {
                d.filing_basis = Form0605FilingBasis::Fiscal;
                d.year_end_month = 12;
            },
            "You have entered invalid month for Fiscal Year",
        );
        check(&|d| d.atc = None, "Please enter a valid ATC on Item 6.");
        check(
            &|d| d.atc = form_0605_atc("PT010"),
            "Please enter a valid ATC on Item 6.",
        );
        check(
            &|d| d.return_period = None,
            "Please enter a valid Return Period on Item 7.",
        );
        check(
            &|d| d.return_period = Form0605Date::new(2026, 1, 31).ok(),
            "The return period date should not be later than the year ended date.",
        );
        check(
            &|d| d.tin = "12345".into(),
            "Please enter a valid TIN number on Item 9.",
        );
        check(
            &|d| d.tin = "123-456-789-00000".into(),
            "You have entered an incorrect TIN",
        );
        check(
            &|d| d.rdo_code = "000".into(),
            "Please enter a valid RDO Code on Item 10.",
        );
        check(
            &|d| d.line_of_business.clear(),
            "Please enter a valid Line of Business/Occupation on Item 12.",
        );
        check(
            &|d| d.taxpayer_name.clear(),
            "Please enter a valid Taxpayer Name on Item 13.",
        );
        check(
            &|d| d.contact_number = "0917-000".into(),
            "Please enter a valid Taxpayer Telephone Number on Item 14.",
        );
        check(
            &|d| d.registered_address.clear(),
            "Please enter a valid Taxpayer Registered Address on Item 15.",
        );
        check(
            &|d| d.zip_code.clear(),
            "Please enter a valid Taxpayer Zip Code on Item 16.",
        );
        check(
            &|d| d.tax_type = None,
            "Please select Tax Type Code on Item 8.",
        );
        check(
            &|d| d.manner_of_payment = None,
            "Please select Manner of Payment on item 17.",
        );
        check(
            &|d| {
                d.manner_of_payment =
                    Some(Form0605MannerOfPayment::PreliminaryOrFinalAssessmentOrDeficiencyTax)
            },
            "Since you have selected Preliminary/Final Assess/Deficiency Tax on\nitem 17, you must choose either:Pre-approved or Not approved byInvestigating Office.",
        );
        check(
            &|d| {
                d.type_of_payment = Some(Form0605TypeOfPayment::Installment);
                d.number_of_installments = Some(21);
            },
            "Please re-enter No. of Installment. Allowed values from 1 to 20 only.",
        );
        check(
            &|d| d.type_of_payment = None,
            "Please select Type of Payment on item 18.",
        );
        check(
            &|d| {
                d.item_19_basic_tax_or_payment = 0.0;
                d.recompute();
            },
            "Please enter valid value (greater than 0) for item 19 under Computation of Tax.",
        );
    }

    #[test]
    fn old_stored_json_still_loads() {
        let mut json = serde_json::to_value(sample()).unwrap();
        let object = json.as_object_mut().unwrap();
        for key in ["queued_submission_fingerprint", "queue_authorization"] {
            object.remove(key);
        }
        for key in [
            "submitted_at",
            "confirmed_at",
            "submission_filename",
            "receipt_id",
            "next_retry_at",
        ] {
            object.insert(key.into(), serde_json::Value::Null);
        }
        object.insert("submission_attempts".into(), serde_json::json!(1));
        object.insert("last_error".into(), serde_json::json!("old error"));
        object.insert("status".into(), serde_json::json!("Draft"));
        object.insert(
            "created_at".into(),
            serde_json::json!("2025-04-01T00:00:00+00:00"),
        );
        object.insert(
            "updated_at".into(),
            serde_json::json!("2025-04-02T00:00:00+00:00"),
        );
        let draft: Form0605Draft = serde_json::from_value(json).unwrap();
        assert_eq!(draft.lifecycle.status, FilingStatus::Draft);
        assert_eq!(draft.lifecycle.created_at, "2025-04-01T00:00:00+00:00");
        assert_eq!(draft.lifecycle.submission_attempts, 1);
        assert_eq!(draft.last_error.as_deref(), Some("old error"));
        let back = serde_json::to_value(&draft).unwrap();
        assert_eq!(back["status"], "Draft");
        assert!(back.get("lifecycle").is_none());
    }

    #[test]
    fn queue_and_revalidate_through_the_generic_path() {
        let mut draft = sample();
        draft.queue(QueueAuthSource::Gui).unwrap();
        assert!(draft.clone().revalidate_queued_before_submission().is_ok());
        draft.item_20b_interest = 9.0;
        draft.keep_entered_penalties();
        assert!(draft.revalidate_queued_before_submission().is_err());
        assert_eq!(draft.lifecycle.status, FilingStatus::Draft);
        assert_eq!(draft.period_column(), 1);
    }

    #[test]
    fn typed_date_alerts_follow_the_official_branches() {
        use super::form_0605_typed_date_alert as alert;
        assert_eq!(alert(4, ""), Some("Invalid date entry on item 4."));
        assert_eq!(alert(4, "  "), Some("Invalid date entry on item 4."));
        assert_eq!(alert(7, "03/31/2025"), None);
        assert_eq!(
            alert(7, "2/05/2025"),
            Some("Please enter a valid day/month on item 7. Format should be MM/DD/YYYY.")
        );
        assert_eq!(
            alert(4, "02/29/2025"),
            Some("Filing year is not a leap year.")
        );
        assert_eq!(
            alert(7, "02/29/2025"),
            Some("Filing year is not a leap year.")
        );
        assert_eq!(alert(7, "02/29/2024"), None);
        assert_eq!(
            alert(7, "00/10/2025"),
            Some("Please enter a valid month on item 7.")
        );
        assert_eq!(
            alert(4, "13/10/2025"),
            Some("Please enter valid month on item 4.")
        );
        assert_eq!(
            alert(4, "01/32/2025"),
            Some("Invalid date entry on item 4.")
        );
        assert_eq!(
            alert(7, "04/00/2025"),
            Some("Invalid date entry on item 7.")
        );
        assert_eq!(
            alert(4, "01/10/"),
            Some("Please enter valid year on item 4.")
        );
        assert_eq!(
            alert(7, "01/10/1800"),
            Some("Please enter a valid date on item 7.")
        );
        assert_eq!(
            alert(7, "01/10"),
            Some("Please enter a valid Return Period on Item 7.")
        );
    }

    /// P5-1: the earlier float-tolerance check refused two-decimal
    /// amounts above about 2,000,000 (`8_816_764.47 * 100` lands 1.2e-7
    /// off a whole number); the shared `official_inputs` check accepts
    /// them again, and a three-decimal amount is still refused.
    #[test]
    fn large_two_decimal_amounts_pass_cent_precision() {
        let mut draft = sample();
        draft.item_20a_surcharge = 8_816_764.47;
        assert!(
            <Form0605Draft as FormValidator>::validate(&draft)
                .iter()
                .all(|(field, _)| field != "item_20a_surcharge"),
            "a large two-decimal amount must be accepted"
        );
        draft.item_20a_surcharge = 1.005;
        assert!(
            <Form0605Draft as FormValidator>::validate(&draft)
                .iter()
                .any(|(field, _)| field == "item_20a_surcharge"),
            "a three-decimal amount must still be refused"
        );
    }
}
