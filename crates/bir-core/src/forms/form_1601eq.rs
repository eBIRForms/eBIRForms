//! BIR Form 1601-EQ — Quarterly Remittance Return of Creditable Income Taxes
//! Withheld (Expanded), January 2018 (ENCS).
//!
//! Ported from the official `BIR-Form1601EQ.hta` (eBIRForms 7.9.6.2.1): the
//! ATC popup (`changedrpATCList` / `getATCCode` / `changeATCRate`), the
//! compute chain (`getRequiredWithheld` … `computeOfTotalAmtDue`),
//! `validateForm` with its exact alert texts, and the uploaded file
//! `saveEncryptedProfile` writes (every `frmMain` control in DOM order).
//!
//! The generated layout (`data/official-xml/1601eq-v2018.json`) already has
//! Items 13–18, which `populateAtcPart2()` draws at load. What the page adds
//! later, [`Form1601EqDraft::official_layout`] adds where the DOM has it:
//! - the popup's ATC checkboxes (`AtcCode1..n`), drawn when Item 11 is
//!   answered, right before `hPartIITableSize`;
//! - "Other Selected ATC" rows 7+ (more than six ATCs), right after
//!   `hPartIITableSize`.

use super::penalty_items::PenaltyItems;
use super::penalty_items::PenaltyState;
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::official_inputs::has_cent_precision;
use super::queueable::{QueueableForm, SubmissionLifecycle};
use super::{FilingPeriod, FormValidator};
use crate::official_xml::{
    Codec, Entry, OfficialLayout, Part, official_amount, parse_official_amount,
};
use crate::profile::TaxpayerProfile;

/// Rule-package id of the official layout.
pub const FORM_1601EQ_FORM_ID: &str = "1601eq-v2018";
/// Items 13–18 on page 1; further ATCs go to "Other Selected ATC".
pub const FORM_1601EQ_MAIN_ROWS: usize = 6;
/// Item number of the first ATC row.
pub const FORM_1601EQ_FIRST_ITEM: usize = 13;
/// The popup list for Item 11 "Government" has the first 96 entries of the
/// private list (with two different rates, see [`FORM_1601EQ_ATC_OPTIONS`]).
pub const FORM_1601EQ_GOVERNMENT_LIST_LEN: usize = 96;

/// One entry of the official ATC popup (`atcCodes.xml`, form 1601EQ).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Form1601EqAtcOption {
    pub code: &'static str,
    pub description: &'static str,
    /// Rate text the popup shows (`txtRate<i>`), e.g. `"5.0"`.
    pub rate_text: &'static str,
}

impl Form1601EqAtcOption {
    /// `changeATCRate`: returns for 2018 and earlier use the old rates of
    /// four MERALCO/DU/interest ATCs.
    pub fn rate_text_for_year(&self, year: u16) -> &'static str {
        if year <= 2018 {
            match self.code {
                "WI650" | "WC650" => return "25.0",
                "WI651" | "WC651" => return "32.0",
                "WI663" | "WC663" | "WI710" | "WC710" => return "20.0",
                _ => {}
            }
        }
        self.rate_text
    }

    pub fn rate_for_year(&self, year: u16) -> f64 {
        self.rate_text_for_year(year).parse().unwrap_or(0.0)
    }
}

/// The popup list in official order (`atcCodes.xml` entries tagged `P`).
///
/// The official page opens the popup with `showPartIIATC(); changeYear();`
/// and `changeYear()` rebuilds the list from
/// `getElementById('frm1601EQ:optCategory').value`. The eBIRForms HTA runs in
/// IE7 mode, where that id lookup also matches by `name` and returns the
/// first Item 11 radio ("Private"). So whichever category is chosen, the ATCs
/// a filer can pick, their order and their rates are this list.
pub const FORM_1601EQ_ATC_OPTIONS: &[Form1601EqAtcOption] = &[
    Form1601EqAtcOption {
        code: "WI010",
        description: "PROFESSIONAL (LAWYERS, CPAS, ENGINEERS, ETC.) - IF GROSS INCOME FOR THE CURRENT YEAR DID NOT EXCEED P 3M",
        rate_text: "5.0",
    },
    Form1601EqAtcOption {
        code: "WI011",
        description: "PROFESSIONAL (LAWYERS, CPAS, ENGINEERS, ETC.) - IF GROSS INCOME IS MORE THAN P 3M OR VAT REGISTERED REGARDLESS OF AMOUNT",
        rate_text: "10.0",
    },
    Form1601EqAtcOption {
        code: "WI020",
        description: "PROFESSIONAL ENTERTAINERS SUCH AS, BUT NOT LIMITED TO ACTORS AND ACTRESSES, SINGERS, LYRICISTS, COMPOSERS, EMCEES - IF GROSS INCOME FOR THE CURRENT YEAR DID NOT EXCEED P 3M",
        rate_text: "5.0",
    },
    Form1601EqAtcOption {
        code: "WI021",
        description: "PROFESSIONAL ENTERTAINERS SUCH AS, BUT NOT LIMITED TO ACTORS AND ACTRESSES, SINGERS, LYRICISTS, COMPOSERS, EMCEES - IF GROSS INCOME IS MORE THAN P 3M OR VAT REGISTERED REGARDLESS OF AMOUNT",
        rate_text: "10.0",
    },
    Form1601EqAtcOption {
        code: "WI030",
        description: "PROFESSIONAL ATHLETES INCLUDING BASKETBALL PLAYERS, PELOTARIS AND JOCKEYS - IF GROSS INCOME FOR THE CURRENT YEAR DID NOT EXCEED P 3M",
        rate_text: "5.0",
    },
    Form1601EqAtcOption {
        code: "WI031",
        description: "PROFESSIONAL ATHLETES INCLUDING BASKETBALL PLAYERS, PELOTARIS AND JOCKEYS - IF GROSS INCOME IS MORE THAN P 3M OR VAT REGISTERED REGARDLESS OF AMOUNT",
        rate_text: "10.0",
    },
    Form1601EqAtcOption {
        code: "WI040",
        description: "ALL DIRECTORS AND PRODUCERS INVOLVED IN MOVIES, STAGE, RADIO, TELEVISION AND MUSICAL PRODUCTIONS - IF GROSS INCOME FOR THE CURRENT YEAR DID NOT EXCEED P 3M",
        rate_text: "5.0",
    },
    Form1601EqAtcOption {
        code: "WI041",
        description: "ALL DIRECTORS AND PRODUCERS INVOLVED IN MOVIES, STAGE, RADIO, TELEVISION AND MUSICAL PRODUCTIONS - IF GROSS INCOME IS MORE THAN P 3M OR VAT REGISTERED REGARDLESS OF AMOUNT",
        rate_text: "10.0",
    },
    Form1601EqAtcOption {
        code: "WI050",
        description: "MANAGEMENT AND TECHNICAL CONSULTANTS - IF GROSS INCOME FOR THE CURRENT YEAR DID NOT EXCEED P 3M",
        rate_text: "5.0",
    },
    Form1601EqAtcOption {
        code: "WI051",
        description: "MANAGEMENT AND TECHNICAL CONSULTANTS - IF GROSS INCOME IS MORE THAN P 3M OR VAT REGISTERED REGARDLESS OF AMOUNT",
        rate_text: "10.0",
    },
    Form1601EqAtcOption {
        code: "WI060",
        description: "BUSINESS AND BOOKKEEPING AGENTS AND AGENCIES - IF GROSS INCOME FOR THE CURRENT YEAR DID NOT EXCEED P 3M",
        rate_text: "5.0",
    },
    Form1601EqAtcOption {
        code: "WI061",
        description: "BUSINESS AND BOOKKEEPING AGENTS AND AGENCIES -IF GROSS INCOME IS MORE THAN P 3M OR VAT REGISTERED REGARDLESS OF AMOUNT",
        rate_text: "10.0",
    },
    Form1601EqAtcOption {
        code: "WI070",
        description: "INSURANCE AGENTS AND INSURANCE ADJUSTERS - IF GROSS INCOME FOR THE CURRENT YEAR DID NOT EXCEED P 3M",
        rate_text: "5.0",
    },
    Form1601EqAtcOption {
        code: "WI071",
        description: "INSURANCE AGENTS AND INSURANCE ADJUSTERS - IF GROSS INCOME IS MORE THAN P 3M OR VAT REGISTERED REGARDLESS OF AMOUNT",
        rate_text: "10.0",
    },
    Form1601EqAtcOption {
        code: "WI080",
        description: "OTHER RECIPIENTS OF TALENT FEES - IF GROSS INCOME FOR THE CURRENT YEAR DID NOT EXCEED P 3M",
        rate_text: "5.0",
    },
    Form1601EqAtcOption {
        code: "WI081",
        description: "OTHER RECIPIENTS OF TALENT FEES - IF GROSS INCOME IS MORE THAN P 3M OR VAT REGISTERED REGARDLESS OF AMOUNT",
        rate_text: "10.0",
    },
    Form1601EqAtcOption {
        code: "WI090",
        description: "FEES OF DIRECTORS WHO ARE NOT EMPLOYEES OF THE COMPANY - IF GROSS INCOME FOR THE CURRENT YEAR DID NOT EXCEED P 3M",
        rate_text: "5.0",
    },
    Form1601EqAtcOption {
        code: "WI091",
        description: "FEES OF DIRECTORS WHO ARE NOT EMPLOYEES OF THE COMPANY - IF GROSS INCOME IS MORE THAN P 3M OR VAT REGISTERED REGARDLESS OF AMOUNT",
        rate_text: "10.0",
    },
    Form1601EqAtcOption {
        code: "WI100",
        description: "RENTALS: ON GROSS RENTAL OR LEASE FOR THE CONTINUED USE OR POSSESSION OF PERSONAL PROPERTY IN EXCESS OF TEN THOUSAND PESOS (P 10,000) ANNUALLY AND REAL PROPERTY USED IN BUSINESS WHICH THE PAYOR OR OBLIGOR HAS NOT TAKEN TITLE OR IS NOT TAKING TITLE, OR IN WHICH HAS NO EQUITY; POLES, SATELLITES , TRANSMISSION FACILITIES AND BILLBOARDS",
        rate_text: "5.0",
    },
    Form1601EqAtcOption {
        code: "WI110",
        description: "CINEMATOGRAPHIC FILM RENTALS AND OTHER PAYMENTS TO RESIDENT INDIVIDUALS AND CORPORATE CINEMATOGRAPHIC FILM OWNERS, LESSORS OR DISTRIBUTORS",
        rate_text: "5.0",
    },
    Form1601EqAtcOption {
        code: "WI120",
        description: "INCOME PAYMENTS TO CERTAIN CONTRACTORS ",
        rate_text: "2.0",
    },
    Form1601EqAtcOption {
        code: "WI130",
        description: "INCOME DISTRIBUTION TO THE BENEFICIARIES OF ESTATES AND TRUSTS",
        rate_text: "15.0",
    },
    Form1601EqAtcOption {
        code: "WI139",
        description: "GROSS COMMISSIONS OR SERVICE FEES OF CUSTOMS, INSURANCE, STOCK, IMMIGRATION AND COMMERCIAL BROKERS, FEES OF AGENTS OF PROFESSIONAL ENTERTAINERS AND REAL ESTATE SERVICE PRACTITIONERS (RESPS), (I.E. REAL ESTATE CONSULTANTS, REAL ESTATE APPRAISERS AND REAL ESTATE BROKERS) - IF GROSS INCOME FOR THE CURRENT YEAR DID NOT EXCEED P 3M",
        rate_text: "5.0",
    },
    Form1601EqAtcOption {
        code: "WI140",
        description: "GROSS COMMISSIONS OR SERVICE FEES OF CUSTOMS, INSURANCE, STOCK, IMMIGRATION AND COMMERCIAL BROKERS, FEES OF AGENTS OF PROFESSIONAL ENTERTAINERS AND REAL ESTATE SERVICE PRACTITIONERS (RESPS), (I.E. REAL ESTATE CONSULTANTS, REAL ESTATE APPRAISERS AND REAL ESTATE BROKERS) -  IF GROSS INCOME IS MORE THAN P 3M OR VAT REGISTERED REGARDLESS OF AMOUNT",
        rate_text: "10.0",
    },
    Form1601EqAtcOption {
        code: "WI150",
        description: "PROFESSIONAL FEES PAID TO MEDICAL PRACTITIONERS (INCLUDES DOCTORS OF MEDICINE, DOCTORS OF VETERINARY SCIENCE & DENTISTS) BY HOSPITALS & CLINICS OR PAID DIRECTLY BY HEALTH MAINTENANCE ORGANIZATIONS (HMOS) AND/OR SIMILAR ESTABLISHMENTS - IF GROSS INCOME IS MORE THAN P 3M OR VAT REGISTERED REGARDLESS OF AMOUNT",
        rate_text: "10.0",
    },
    Form1601EqAtcOption {
        code: "WI151",
        description: "PROFESSIONAL FEES PAID TO MEDICAL PRACTITIONERS (INCLUDES DOCTORS OF MEDICINE, DOCTORS OF VETERINARY SCIENCE & DENTISTS) BY HOSPITALS & CLINICS OR PAID DIRECTLY BY HEALTH MAINTENANCE ORGANIZATIONS (HMOS) AND/OR SIMILAR ESTABLISHMENTS - IF GROSS INCOME FOR THE CURRENT YEAR DID NOT EXCEED P 3M",
        rate_text: "5.0",
    },
    Form1601EqAtcOption {
        code: "WI152",
        description: "PAYMENT BY THE GENERAL PROFESSIONAL PARTNERSHIPS (GPPS) TO ITS PARTNERS - IF GROSS INCOME FOR THE CURRENT YEAR DID NOT EXCEED P 720,000",
        rate_text: "10.0",
    },
    Form1601EqAtcOption {
        code: "WI153",
        description: "PAYMENT BY THE GENERAL PROFESSIONAL PARTNERSHIPS (GPPS) TO ITS PARTNERS - IF GROSS INCOME EXCEEDS P 720,000",
        rate_text: "15.0",
    },
    Form1601EqAtcOption {
        code: "WI156",
        description: "INCOME PAYMENTS MADE BY CREDIT CARD COMPANIES",
        rate_text: "0.5",
    },
    Form1601EqAtcOption {
        code: "WI640",
        description: "INCOME PAYMENTS MADE BY THE GOVERNMENT AND GOVERNMENT-OWNED AND CONTROLLED CORPORATIONS (GOCCS) TO ITS LOCAL/RESIDENT SUPPLIERS OF GOODS OTHER THAN THOSE COVERED BY OTHER RATES OF WITHHOLDING TAX",
        rate_text: "1.0",
    },
    Form1601EqAtcOption {
        code: "WI157",
        description: "INCOME PAYMENTS MADE BY THE GOVERNMENT AND GOVERNMENT-OWNED AND CONTROLLED CORPORATIONS (GOCCS) TO ITS LOCAL/RESIDENT SUPPLIERS OF SERVICES OTHER THAN THOSE COVERED BY OTHER RATES OF WITHHOLDING TAX",
        rate_text: "2.0",
    },
    Form1601EqAtcOption {
        code: "WI159",
        description: "ADDITIONAL INCOME PAYMENTS TO GOVERNMENT PERSONNEL FROM IMPORTERS, SHIPPING AND AIRLINE COMPANIES OR THEIR AGENTS FOR OVERTIME SERVICES",
        rate_text: "15.0",
    },
    Form1601EqAtcOption {
        code: "WI158",
        description: "INCOME PAYMENT MADE BY TOP WITHHOLDING AGENTS TO THEIR LOCAL/RESIDENT SUPPLIER OF GOODS OTHER THAN THOSE COVERED BY OTHER RATES OF WITHHOLDING TAX",
        rate_text: "1.0",
    },
    Form1601EqAtcOption {
        code: "WI160",
        description: "INCOME PAYMENT MADE BY TOP WITHHOLDING AGENTS TO THEIR LOCAL/RESIDENT SUPPLIER OF SERVICES OTHER THAN THOSE COVERED BY OTHER RATES OF WITHHOLDING TAX",
        rate_text: "2.0",
    },
    Form1601EqAtcOption {
        code: "WI515",
        description: "COMMISSIONS, REBATES, DISCOUNTS AND OTHER SIMILAR CONSIDERATIONS PAID/GRANTED TO INDEPENDENT AND/OR EXCLUSIVE SALES REPRESENTATIVES AND MARKETING AGENTS AND SUB-AGENTS OF COMPANIES, INCLUDING MULTI-LEVEL MARKETING COMPANIES - IF GROSS INCOME FOR THE CURRENT YEAR DID NOT EXCEED P 3M",
        rate_text: "5.0",
    },
    Form1601EqAtcOption {
        code: "WI516",
        description: "COMMISSIONS, REBATES, DISCOUNTS AND OTHER SIMILAR CONSIDERATIONS PAID/GRANTED TO INDEPENDENT AND/OR EXCLUSIVE SALES REPRESENTATIVES AND MARKETING AGENTS AND SUB-AGENTS OF COMPANIES, INCLUDING MULTI-LEVEL MARKETING COMPANIES - IF GROSS INCOME IS MORE THAN P 3M OR VAT REGISTERED REGARDLESS OF AMOUNT",
        rate_text: "10.0",
    },
    Form1601EqAtcOption {
        code: "WI530",
        description: "GROSS PAYMENTS TO EMBALMERS BY FUNERAL PARLORS",
        rate_text: "1.0",
    },
    Form1601EqAtcOption {
        code: "WI535",
        description: "PAYMENTS MADE BY PRE-NEED COMPANIES TO FUNERAL PARLORS",
        rate_text: "1.0",
    },
    Form1601EqAtcOption {
        code: "WI540",
        description: "TOLLING FEES PAID TO REFINERIES",
        rate_text: "5.0",
    },
    Form1601EqAtcOption {
        code: "WI610",
        description: "INCOME PAYMENTS MADE TO SUPPLIERS OF AGRICULTURAL PRODUCTS IN EXCESS OF CUMULATIVE AMOUNT OF P 300,000 WITHIN THE SAME TAXABLE YEAR",
        rate_text: "1.0",
    },
    Form1601EqAtcOption {
        code: "WI630",
        description: "INCOME PAYMENTS ON PURCHASES OF MINERALS, MINERAL PRODUCTS AND QUARRY RESOURCES, SUCH AS BUT NOT LIMITED TO SILVER, GOLD, MARBLE, GRANITE, GRAVEL, SAND, BOULDERS AND OTHER MINERAL PRODUCTS EXCEPT PURCHASES BY BANGKO SENTRAL NG PILIPINAS ",
        rate_text: "5.0",
    },
    Form1601EqAtcOption {
        code: "WI632",
        description: "INCOME PAYMENTS ON PURCHASES OF MINERALS, MINERAL PRODUCTS AND QUARRY RESOURCES BY BANGKO SENTRAL NG PILIPINAS (BSP) FROM GOLD MINERS/SUPPLIERS UNDER PD 1899, AS AMENDED BY RA NO. 7076",
        rate_text: "1.0",
    },
    Form1601EqAtcOption {
        code: "WI650",
        description: "ON GROSS AMOUNT OF REFUND GIVEN BY MERALCO TO CUSTOMERS WITH ACTIVE CONTRACTS AS CLASSIFIED BY MERALCO",
        rate_text: "15.0",
    },
    Form1601EqAtcOption {
        code: "WI651",
        description: "ON GROSS AMOUNT OF REFUND GIVEN BY MERALCO TO CUSTOMERS WITH TERMINATED CONTRACTS AS CLASSIFIED BY MERALCO",
        rate_text: "15.0",
    },
    Form1601EqAtcOption {
        code: "WI660",
        description: "ON GROSS AMOUNT OF INTEREST ON THE REFUND OF METER DEPOSIT WHETHER PAID DIRECTLY TO THE CUSTOMERS OR APPLIED AGAINST CUSTOMER'S BILLINGS OF RESIDENTIAL AND GENERAL SERVICE CUSTOMERS WHOSE MONTHLY ELECTRICITY CONSUMPTION EXCEEDS 200 KWH AS CLASSIFIED BY MERALCO",
        rate_text: "10.0",
    },
    Form1601EqAtcOption {
        code: "WI661",
        description: "ON GROSS AMOUNT OF INTEREST ON THE REFUND OF METER DEPOSIT WHETHER PAID DIRECTLY TO THE CUSTOMERS OR APPLIED AGAINST CUSTOMER'S BILLINGS OF  NON-RESIDENTIAL CUSTOMERS WHOSE MONTHLY ELECTRICITY CONSUMPTION EXCEEDS 200 KWH AS CLASSIFIED BY MERALCO",
        rate_text: "10.0",
    },
    Form1601EqAtcOption {
        code: "WI662",
        description: "ON GROSS AMOUNT OF INTEREST ON THE REFUND OF METER DEPOSIT WHETHER PAID DIRECTLY TO THE CUSTOMERS OR APPLIED AGAINST CUSTOMER'S BILLINGS OF RESIDENTIAL AND GENERAL SERVICE CUSTOMERS WHOSE MONTHLY ELECTRICITY CONSUMPTION EXCEEDS 200 KWH AS CLASSIFIED BY OTHER ELECTRIC DISTRIBUTION UTILITIES (DU)",
        rate_text: "10.0",
    },
    Form1601EqAtcOption {
        code: "WI663",
        description: "ON GROSS AMOUNT OF INTEREST ON THE REFUND OF METER DEPOSIT WHETHER PAID DIRECTLY TO THE CUSTOMERS OR APPLIED AGAINST CUSTOMER'S ILLINGS OF  NON-RESIDENTIAL CUSTOMERS WHOSE MONTHLY ELECTRICITY CONSUMPTION EXCEEDS 200 KWH AS CLASSIFIED BY OTHER ELECTRIC DISTRIBUTION UTILITIES (DU)",
        rate_text: "15.0",
    },
    Form1601EqAtcOption {
        code: "WI680",
        description: "INCOME PAYMENTS MADE BY POLITICAL PARTIES AND CANDIDATES OF LOCAL AND NATIONAL ELECTIONS ON ALL THEIR PURCHASES OF GOODS AND SERVICES RELATED TO CAMPAIGN EXPENDITURES, AND INCOME PAYMENTS MADE BY INDIVIDUALS OR JURIDICAL PERSONS FOR THEIR PURCHASES OF GOODS AND SERVICES INTENDED TO BE GIVEN AS CAMPAIGN CONTRIBUTIONS TO POLITICAL PARTIES AND CANDIDATES",
        rate_text: "5.0",
    },
    Form1601EqAtcOption {
        code: "WI710",
        description: "INTEREST INCOME DERIVED FROM ANY OTHER DEBT INSTRUMENTS NOT WITHIN THE COVERAGE OF DEPOSIT SUBSTITUTES AND REVENUE REGULATIONS NO. 14-2012 ",
        rate_text: "15.0",
    },
    Form1601EqAtcOption {
        code: "WI720",
        description: "INCOME PAYMENTS ON LOCALLY PRODUCED RAW SUGAR",
        rate_text: "1.0",
    },
    Form1601EqAtcOption {
        code: "WC010",
        description: "PROFESSIONAL (LAWYERS, CPAS, ENGINEERS, ETC.) - IF GROSS INCOME FOR THE CURRENT YEAR DID NOT EXCEED P 720,000",
        rate_text: "10.0",
    },
    Form1601EqAtcOption {
        code: "WC011",
        description: "PROFESSIONAL (LAWYERS, CPAS, ENGINEERS, ETC.) - IF GROSS INCOME EXCEEDS P 720,000",
        rate_text: "15.0",
    },
    Form1601EqAtcOption {
        code: "WC020",
        description: "PROFESSIONAL ENTERTAINERS SUCH AS, BUT NOT LIMITED TO ACTORS AND ACTRESSES, SINGERS, LYRICISTS, COMPOSERS, EMCEES - IF GROSS INCOME FOR THE CURRENT YEAR DID NOT EXCEED P 720,000",
        rate_text: "10.0",
    },
    Form1601EqAtcOption {
        code: "WC021",
        description: "PROFESSIONAL ENTERTAINERS SUCH AS, BUT NOT LIMITED TO ACTORS AND ACTRESSES, SINGERS, LYRICISTS, COMPOSERS, EMCEES - IF GROSS INCOME EXCEEDS P 720,000",
        rate_text: "15.0",
    },
    Form1601EqAtcOption {
        code: "WC030",
        description: "PROFESSIONAL ATHLETES INCLUDING BASKETBALL PLAYERS, PELOTARIS AND JOCKEYS - IF GROSS INCOME FOR THE CURRENT YEAR DID NOT EXCEED P 720,000",
        rate_text: "10.0",
    },
    Form1601EqAtcOption {
        code: "WC031",
        description: "PROFESSIONAL ATHLETES INCLUDING BASKETBALL PLAYERS, PELOTARIS AND JOCKEYS - IF GROSS INCOME EXCEEDS P 720,000",
        rate_text: "15.0",
    },
    Form1601EqAtcOption {
        code: "WC040",
        description: "ALL DIRECTORS AND PRODUCERS INVOLVED IN MOVIES, STAGE, RADIO, TELEVISION AND MUSICAL PRODUCTIONS - IF GROSS INCOME FOR THE CURRENT YEAR DID NOT EXCEED P 720,000",
        rate_text: "10.0",
    },
    Form1601EqAtcOption {
        code: "WC041",
        description: "ALL DIRECTORS AND PRODUCERS INVOLVED IN MOVIES, STAGE, RADIO, TELEVISION AND MUSICAL PRODUCTIONS -  IF GROSS INCOME EXCEEDS P 720,000",
        rate_text: "15.0",
    },
    Form1601EqAtcOption {
        code: "WC050",
        description: "MANAGEMENT AND TECHNICAL CONSULTANTS - IF GROSS INCOME FOR THE CURRENT YEAR DID NOT EXCEED P 720,000",
        rate_text: "10.0",
    },
    Form1601EqAtcOption {
        code: "WC051",
        description: "MANAGEMENT AND TECHNICAL CONSULTANTS - IF GROSS INCOME EXCEEDS P 720,000",
        rate_text: "15.0",
    },
    Form1601EqAtcOption {
        code: "WC060",
        description: "BUSINESS AND BOOKKEEPING AGENTS AND AGENCIES - IF GROSS INCOME FOR THE CURRENT YEAR DID NOT EXCEED P 720,000",
        rate_text: "10.0",
    },
    Form1601EqAtcOption {
        code: "WC061",
        description: "BUSINESS AND BOOKKEEPING AGENTS AND AGENCIES - IF GROSS INCOME EXCEEDS P 720,000",
        rate_text: "15.0",
    },
    Form1601EqAtcOption {
        code: "WC070",
        description: "INSURANCE AGENTS AND INSURANCE ADJUSTERS - IF GROSS INCOME FOR THE CURRENT YEAR DID NOT EXCEED P 720,000",
        rate_text: "10.0",
    },
    Form1601EqAtcOption {
        code: "WC071",
        description: "INSURANCE AGENTS AND INSURANCE ADJUSTERS - IF GROSS INCOME EXCEEDS P 720,000",
        rate_text: "15.0",
    },
    Form1601EqAtcOption {
        code: "WC080",
        description: "OTHER RECIPIENTS OF TALENT FEES - IF GROSS INCOME FOR THE CURRENT YEAR DID NOT EXCEED P 720,000",
        rate_text: "10.0",
    },
    Form1601EqAtcOption {
        code: "WC081",
        description: "OTHER RECIPIENTS OF TALENT FEES - IF GROSS INCOME EXCEEDS P 720,000",
        rate_text: "15.0",
    },
    Form1601EqAtcOption {
        code: "WC100",
        description: "RENTALS: ON GROSS RENTAL OR LEASE FOR THE CONTINUED USE OR POSSESSION OF PERSONAL PROPERTY IN EXCESS OF TEN THOUSAND PESOS (P 10,000) ANNUALLY AND REAL PROPERTY USED IN BUSINESS WHICH THE PAYOR OR OBLIGOR HAS NOT TAKEN TITLE OR IS NOT TAKING TITLE, OR IN WHICH HAS NO EQUITY; POLES, SATELLITES, TRANSMISSION FACILITIES AND BILLBOARDS",
        rate_text: "5.0",
    },
    Form1601EqAtcOption {
        code: "WC110",
        description: "CINEMATOGRAPHIC FILM RENTALS AND OTHER PAYMENTS TO RESIDENT INDIVIDUALS AND CORPORATE CINEMATOGRAPHIC FILM OWNERS, LESSORS OR DISTRIBUTORS",
        rate_text: "5.0",
    },
    Form1601EqAtcOption {
        code: "WC120",
        description: "INCOME PAYMENTS TO CERTAIN CONTRACTORS ",
        rate_text: "2.0",
    },
    Form1601EqAtcOption {
        code: "WC139",
        description: "GROSS COMMISSIONS OR SERVICE FEES OF CUSTOMS, INSURANCE, STOCK, IMMIGRATION AND COMMERCIAL BROKERS, FEES OF AGENTS OF PROFESSIONAL ENTERTAINERS AND REAL ESTATE SERVICE PRACTITIONERS (RESPS), (I.E. REAL ESTATE ONSULTANTS, REAL ESTATE APPRAISERS AND REAL ESTATE BROKERS) - IF GROSS INCOME FOR THE CURRENT YEAR DID NOT EXCEED P 720,000",
        rate_text: "10.0",
    },
    Form1601EqAtcOption {
        code: "WC140",
        description: "GROSS COMMISSIONS OR SERVICE FEES OF CUSTOMS, INSURANCE, STOCK, IMMIGRATION AND COMMERCIAL BROKERS, FEES OF AGENTS OF PROFESSIONAL ENTERTAINERS AND REAL ESTATE SERVICE PRACTITIONERS (RESPS), (I.E. REAL ESTATE ONSULTANTS, REAL ESTATE APPRAISERS AND REAL ESTATE BROKERS) - IF GROSS INCOME EXCEEDS P 720,000",
        rate_text: "15.0",
    },
    Form1601EqAtcOption {
        code: "WC150",
        description: "PROFESSIONAL FEES PAID TO MEDICAL PRACTITIONERS (INCLUDES DOCTORS OF MEDICINE DOCTORS OF VETERINARY SCIENCE & DENTISTS) BY HOSPITALS & CLINICS OR PAID DIRECTLY BY HEALTH MAINTENANCE ORGANIZATIONS (HMOS) AND/OR SIMILAR ESTABLISHMENTS - IF GROSS INCOME EXCEEDS P 720,000",
        rate_text: "15.0",
    },
    Form1601EqAtcOption {
        code: "WC151",
        description: "PROFESSIONAL FEES PAID TO MEDICAL PRACTITIONERS (INCLUDES DOCTORS OF MEDICINE DOCTORS OF VETERINARY SCIENCE & DENTISTS) BY HOSPITALS & CLINICS OR PAID DIRECTLY BY HEALTH MAINTENANCE ORGANIZATIONS (HMOS) AND/OR SIMILAR ESTABLISHMENTS - IF GROSS INCOME FOR THE CURRENT YEAR DID NOT EXCEED P 720,000",
        rate_text: "10.0",
    },
    Form1601EqAtcOption {
        code: "WC156",
        description: "INCOME PAYMENTS MADE BY CREDIT CARD COMPANIES",
        rate_text: "0.5",
    },
    Form1601EqAtcOption {
        code: "WC640",
        description: "INCOME PAYMENTS MADE BY THE GOVERNMENT AND GOVERNMENT-OWNED AND CONTROLLED CORPORATIONS (GOCCS) TO ITS LOCAL/RESIDENT SUPPLIERS OF GOODS OTHER THAN THOSE COVERED BY OTHER RATES OF WITHHOLDING TAX",
        rate_text: "1.0",
    },
    Form1601EqAtcOption {
        code: "WC157",
        description: "INCOME PAYMENTS MADE BY THE GOVERNMENT AND GOVERNMENT-OWNED AND CONTROLLED CORPORATIONS (GOCCS) TO ITS LOCAL/RESIDENT SUPPLIERS OF SERVICES OTHER THAN THOSE COVERED BY OTHER RATES OF WITHHOLDING TAX",
        rate_text: "2.0",
    },
    Form1601EqAtcOption {
        code: "WC158",
        description: "INCOME PAYMENT MADE BY TOP WITHHOLDING AGENTS TO THEIR LOCAL/RESIDENT SUPPLIER OF GOODS OTHER THAN THOSE COVERED BY OTHER RATES OF WITHHOLDING TAX",
        rate_text: "1.0",
    },
    Form1601EqAtcOption {
        code: "WC160",
        description: "INCOME PAYMENT MADE BY TOP WITHHOLDING AGENTS TO THEIR  LOCAL/RESIDENT SUPPLIER OF SERVICES OTHER THAN THOSE COVERED BY OTHER RATES OF WITHHOLDING TAX",
        rate_text: "2.0",
    },
    Form1601EqAtcOption {
        code: "WC515",
        description: "COMMISSIONS, REBATES, DISCOUNTS AND OTHER SIMILAR CONSIDERATIONS PAID/ GRANTED TO INDEPENDENT AND/OR EXCLUSIVE SALES REPRESENTATIVES AND MARKETING AGENTS AND SUB-AGENTS OF COMPANIES, INCLUDING MULTI-LEVEL MARKETING COMPANIES - IF GROSS INCOME FOR THE CURRENT YEAR DID NOT EXCEED P 720,000",
        rate_text: "10.0",
    },
    Form1601EqAtcOption {
        code: "WC516",
        description: "COMMISSIONS, REBATES, DISCOUNTS AND OTHER SIMILAR CONSIDERATIONS PAID/ GRANTED TO INDEPENDENT AND/OR EXCLUSIVE SALES REPRESENTATIVES AND MARKETING AGENTS AND SUB-AGENTS OF COMPANIES, INCLUDING MULTI-LEVEL MARKETING COMPANIES - IF GROSS INCOME EXCEEDS P 720,000",
        rate_text: "15.0",
    },
    Form1601EqAtcOption {
        code: "WC535",
        description: "PAYMENTS MADE BY PRE-NEED COMPANIES TO FUNERAL PARLORS",
        rate_text: "1.0",
    },
    Form1601EqAtcOption {
        code: "WC540",
        description: "TOLLING FEES PAID TO REFINERIES",
        rate_text: "5.0",
    },
    Form1601EqAtcOption {
        code: "WC610",
        description: "INCOME PAYMENTS MADE TO SUPPLIERS OF AGRICULTURAL PRODUCTS IN EXCESS OF CUMULATIVE AMOUNT OF P 300,000 WITHIN THE SAME TAXABLE YEAR",
        rate_text: "1.0",
    },
    Form1601EqAtcOption {
        code: "WC630",
        description: "INCOME PAYMENTS ON PURCHASES OF MINERALS, MINERAL PRODUCTS AND QUARRY RESOURCES, SUCH AS BUT NOT LIMITED TO SILVER, GOLD, MARBLE, GRANITE, GRAVEL, SAND, BOULDERS AND OTHER MINERAL PRODUCTS EXCEPT PURCHASES BY BANGKO SENTRAL NG PILIPINAS",
        rate_text: "5.0",
    },
    Form1601EqAtcOption {
        code: "WC632",
        description: "INCOME PAYMENTS ON PURCHASES OF MINERALS, MINERAL PRODUCTS AND QUARRY RESOURCES BY BANGKO SENTRAL NG PILIPINAS (BSP) FROM GOLD MINERS/SUPPLIERS UNDER PD 1899, AS AMENDED BY RA NO. 7076",
        rate_text: "1.0",
    },
    Form1601EqAtcOption {
        code: "WC650",
        description: "ON GROSS AMOUNT OF REFUND GIVEN BY MERALCO TO CUSTOMERS WITH ACTIVE CONTRACTS AS CLASSIFIED BY MERALCO",
        rate_text: "15.0",
    },
    Form1601EqAtcOption {
        code: "WC651",
        description: "ON GROSS AMOUNT OF REFUND GIVEN BY MERALCO TO CUSTOMERS WITH TERMINATED CONTRACTS AS CLASSIFIED BY MERALCO",
        rate_text: "15.0",
    },
    Form1601EqAtcOption {
        code: "WC660",
        description: "ON GROSS AMOUNT OF INTEREST ON THE REFUND OF METER DEPOSIT WHETHER PAID DIRECTLY TO THE CUSTOMERS OR APPLIED AGAINST CUSTOMER'S BILLINGS OF RESIDENTIAL AND GENERAL SERVICE CUSTOMERS WHOSE MONTHLY ELECTRICITY CONSUMPTION EXCEEDS 200 KWH AS CLASSIFIED BY MERALCO",
        rate_text: "10.0",
    },
    Form1601EqAtcOption {
        code: "WC661",
        description: "ON GROSS AMOUNT OF INTEREST ON THE REFUND OF METER DEPOSIT WHETHER PAID DIRECTLY TO THE CUSTOMERS OR APPLIED AGAINST CUSTOMER'S BILLINGS OF  NON-RESIDENTIAL CUSTOMERS WHOSE MONTHLY ELECTRICITY CONSUMPTION EXCEEDS 200 KWH AS CLASSIFIED BY MERALCO",
        rate_text: "10.0",
    },
    Form1601EqAtcOption {
        code: "WC662",
        description: "ON GROSS AMOUNT OF INTEREST ON THE REFUND OF METER DEPOSIT WHETHER PAID DIRECTLY TO THE CUSTOMERS OR APPLIED AGAINST CUSTOMER'S BILLINGS OF RESIDENTIAL AND GENERAL SERVICE CUSTOMERS WHOSE MONTHLY ELECTRICITY CONSUMPTION EXCEEDS 200 KWH AS CLASSIFIED BY OTHER ELECTRIC DISTRIBUTION UTILITIES (DU)",
        rate_text: "10.0",
    },
    Form1601EqAtcOption {
        code: "WC663",
        description: "ON GROSS AMOUNT OF INTEREST ON THE REFUND OF METER DEPOSIT WHETHER PAID DIRECTLY TO THE CUSTOMERS OR APPLIED AGAINST CUSTOMER'S BILLINGS OF  NON- RESIDENTIAL CUSTOMERS WHOSE MONTHLY ELECTRICITY CONSUMPTION EXCEEDS 200 KWH AS CLASSIFIED BY OTHER ELECTRIC DISTRIBUTION UTILITIES (DU)",
        rate_text: "15.0",
    },
    Form1601EqAtcOption {
        code: "WC680",
        description: "INCOME PAYMENTS MADE BY POLITICAL PARTIES AND CANDIDATES OF LOCAL AND NATIONAL ELECTIONS ON ALL THEIR PURCHASES OF GOODS AND SERVICES RELATED TO CAMPAIGN EXPENDITURES, AND INCOME PAYMENTS MADE BY INDIVIDUALS OR JURIDICAL PERSONS FOR THEIR PURCHASES OF GOODS AND SERVICES INTENDED TO BE GIVEN AS CAMPAIGN CONTRIBUTIONS TO POLITICAL PARTIES AND CANDIDATES",
        rate_text: "5.0",
    },
    Form1601EqAtcOption {
        code: "WC690",
        description: "INCOME PAYMENTS RECEIVED BY REAL ESTATE INVESTMENT TRUST (REIT)",
        rate_text: "1.0",
    },
    Form1601EqAtcOption {
        code: "WC710",
        description: "INTEREST INCOME DERIVED FROM ANY OTHER DEBT INSTRUMENTS NOT WITHIN THE COVERAGE OF DEPOSIT SUBSTITUTES AND REVENUE REGULATIONS NO. 14-2012 ",
        rate_text: "15.0",
    },
    Form1601EqAtcOption {
        code: "WC720",
        description: "INCOME PAYMENTS ON LOCALLY PRODUCED RAW SUGAR",
        rate_text: "1.0",
    },
    Form1601EqAtcOption {
        code: "WI820",
        description: "ON THE GROSS REMITTANCES BY E-MARKETPLACE OPERATORS TO THE SELLERS/MERCHANTS FOR THE GOODS OR SERVICES SOLD/PAID THROUGH THEIR PLATFORM/FACILITY-INDIVIDUAL.",
        rate_text: "0.5",
    },
    Form1601EqAtcOption {
        code: "WI830",
        description: "ON THE GROSS REMITTANCES BY DIGITAL FINANCIAL SERVICES PROVIDERS TO THE SELLERS/MERCHANTS FOR THE GOODS OR SERVICES SOLD/PAID THROUGH THEIR PLATFORM/FACILITY-INDIVIDUAL",
        rate_text: "0.5",
    },
    Form1601EqAtcOption {
        code: "WC820",
        description: "ON THE GROSS REMITTANCES BY E-MARKETPLACE OPERATORS TO THE SELLERS/MERCHANTS FOR THE GOODS OR SERVICES SOLD/PAID THROUGH THEIR PLATFORM/FACILITY-CORPORATE",
        rate_text: "0.5",
    },
    Form1601EqAtcOption {
        code: "WC830",
        description: "ON THE GROSS REMITTANCES BY DIGITAL FINANCIAL SERVICES PROVIDERS TO THE SELLERS/MERCHANTS FOR THE GOODS OR SERVICES SOLD/PAID THROUGH THEIR PLATFORM/FACILITY-CORPORATE",
        rate_text: "0.5",
    },
    Form1601EqAtcOption {
        code: "WI770",
        description: "INCOME PAYMENTS MADE BY JOINT VENTURES, WHETHER INCORPORATED OR NOT, TAXABLE OR NON-TAXABLE, TO THEIR LOCAL/RESIDENT SUPPLIER OF GOODS. INDIVIDUAL",
        rate_text: "1.0",
    },
    Form1601EqAtcOption {
        code: "WI780",
        description: "INCOME PAYMENTS MADE BY JOINT VENTURES, WHETHER INCORPORATED OR NOT, TAXABLE OR NON-TAXABLE, TO THEIR LOCAL/RESIDENT SUPPLIER OF SERVICES. INDIVIDUAL",
        rate_text: "2.0",
    },
    Form1601EqAtcOption {
        code: "WC770",
        description: "INCOME PAYMENTS MADE BY JOINT VENTURES, WHETHER INCORPORATED OR NOT, TAXABLE OR NON-TAXABLE, TO THEIR LOCAL/RESIDENT SUPPLIER OF GOODS. CORPORATE",
        rate_text: "1.0",
    },
    Form1601EqAtcOption {
        code: "WC780",
        description: "INCOME PAYMENTS MADE BY JOINT VENTURES, WHETHER INCORPORATED OR NOT, TAXABLE OR NON-TAXABLE, TO THEIR LOCAL/RESIDENT SUPPLIER OF SERVICES. CORPORATE",
        rate_text: "2.0",
    },
    Form1601EqAtcOption {
        code: "WC790",
        description: "ON THE SHARE OF EACH CO-VENTURER/MEMBER FROM THE NET INCOME OF THE JOINT VENTURE/CONSORTIUM NOT TAXABLE AS CORPORATION PRIOR TO ACTUAL OR CONSTRUCTIVE DISTRIBUTION THEREOF. CORPORATE",
        rate_text: "15.0",
    },
    Form1601EqAtcOption {
        code: "WI840",
        description: "INCOME PAYMENTS MADE BY TOP WITHHOLDING AGENTS, EITHER PRIVATE CORPORATION OR INDIVIDUALS, TO THE MANUFACTURERS AND DIRECT IMPORTERS OF MOTOR VEHICLES IN COMPLETELY BUILT UNITS (CBUs) OR SEMI-KNOCKDOWN (SKD) UNITS, MOTOR VEHICLE PARTS AND ACCESSORIES.",
        rate_text: "0.5",
    },
    Form1601EqAtcOption {
        code: "WC840",
        description: "INCOME PAYMENTS MADE BY TOP WITHHOLDING AGENTS, EITHER PRIVATE CORPORATION OR INDIVIDUALS, TO THE MANUFACTURERS AND DIRECT IMPORTERS OF MOTOR VEHICLES IN COMPLETELY BUILT UNITS (CBUs) OR SEMI-KNOCKDOWN (SKD) UNITS, MOTOR VEHICLE PARTS AND ACCESSORIES.",
        rate_text: "0.5",
    },
    Form1601EqAtcOption {
        code: "WI850",
        description: "INCOME PAYMENTS MADE BY TOP WITHHOLDING AGENTS, EITHER PRIVATE CORPORATION OR INDIVIDUALS, TO THE MANUFACTURERS AND DIRECT IMPORTERS OF MEDICINE/PHARMACEUTICAL PRODUCTS.",
        rate_text: "0.5",
    },
    Form1601EqAtcOption {
        code: "WC850",
        description: "INCOME PAYMENTS MADE BY TOP WITHHOLDING AGENTS, EITHER PRIVATE CORPORATION OR INDIVIDUALS, TO THE MANUFACTURERS AND DIRECT IMPORTERS OF MEDICINE/PHARMACEUTICAL PRODUCTS.",
        rate_text: "0.5",
    },
    Form1601EqAtcOption {
        code: "WI860",
        description: "INCOME PAYMENTS MADE BY TOP WITHHOLDING AGENTS, EITHER PRIVATE CORPORATION OR INDIVIDUALS, TO THE MANUFACTURERS AND DIRECT IMPORTERS OF SOLID OR LIQUID FUELS AND RELATED PRODUCTS.",
        rate_text: "0.5",
    },
    Form1601EqAtcOption {
        code: "WC860",
        description: "INCOME PAYMENTS MADE BY TOP WITHHOLDING AGENTS, EITHER PRIVATE CORPORATION OR INDIVIDUALS, TO THE MANUFACTURERS AND DIRECT IMPORTERS OF SOLID OR LIQUID FUELS AND RELATED PRODUCTS.",
        rate_text: "0.5",
    },
];

/// The official popup option for an ATC code.
pub fn form_1601eq_atc_option(code: &str) -> Option<(usize, &'static Form1601EqAtcOption)> {
    FORM_1601EQ_ATC_OPTIONS
        .iter()
        .enumerate()
        .find(|(_, option)| option.code == code)
}

/// Item 11, category of withholding agent (`optCategory:P` / `:G`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form1601EqCategory {
    Private,
    Government,
}

/// Item 30 boxes (`ifRefund` / `ifIssueCert` / `ifCarriedOver`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form1601EqOverRemittance {
    #[default]
    None,
    Refund,
    TaxCreditCertificate,
    CarriedOver,
}

/// One ATC row: Items 13–18 (and "Other Selected ATC" beyond six).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Form1601EqAtcRow {
    pub atc_code: String,
    /// Tax base consolidated for the quarter.
    pub tax_base: f64,
    /// Tax withheld consolidated for the quarter (computed).
    #[serde(default)]
    pub tax_withheld: f64,
}

impl Form1601EqAtcRow {
    pub fn new(code: &str) -> Self {
        Self {
            atc_code: code.to_string(),
            ..Self::default()
        }
    }

    fn option(&self) -> Option<(usize, &'static Form1601EqAtcOption)> {
        form_1601eq_atc_option(&self.atc_code)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Form1601EqDraft {
    #[serde(default)]
    pub id: Option<i64>,

    /// 14 digits: 9-digit TIN then the 5-digit branch code.
    pub tin: String,
    /// Item 1.
    pub taxable_year: u16,
    /// Item 2 (1–4); 0 while unanswered.
    pub quarter: u8,
    /// Item 3.
    pub is_amended: bool,
    /// Item 4, "Any taxes withheld?"; `None` while unanswered.
    #[serde(default)]
    pub any_tax_withheld: Option<bool>,
    /// Item 5.
    #[serde(default)]
    pub number_of_attached_sheets: u16,

    // Part I
    pub rdo_code: String,
    /// Not printed on the form; the page keeps it in a hidden field.
    pub line_of_business: String,
    pub taxpayer_name: String,
    pub registered_address: String,
    pub zip_code: String,
    pub contact_number: String,
    /// Item 11; `None` while unanswered.
    #[serde(default)]
    pub category: Option<Form1601EqCategory>,
    pub email: String,

    // Part II
    /// ATC rows in popup order (the page always lists them that way).
    #[serde(default)]
    pub schedule: Vec<Form1601EqAtcRow>,
    /// "Other Selected ATC" total (rows beyond six).
    #[serde(default)]
    pub total_other_tax_withheld: f64,
    /// Item 19.
    #[serde(default)]
    pub total_taxes_withheld: f64,
    /// Items 20–23.
    #[serde(default)]
    pub remittance_first_month: f64,
    #[serde(default)]
    pub remittance_second_month: f64,
    #[serde(default)]
    pub tax_remitted_previous: f64,
    #[serde(default)]
    pub over_remittance_previous_quarter: f64,
    /// Item 24.
    #[serde(default)]
    pub total_remittances: f64,
    /// Item 25.
    #[serde(default)]
    pub tax_still_due: f64,
    /// Items 26–29.
    #[serde(default)]
    pub surcharge: f64,
    #[serde(default)]
    pub interest: f64,
    #[serde(default)]
    pub compromise: f64,
    /// The penalty items' computed values, overrides, note and engine
    /// inputs (the shared model in [`super::penalty_items`]).
    #[serde(flatten)]
    pub penalty: PenaltyState,
    #[serde(default)]
    pub total_penalties: f64,
    /// Item 30.
    #[serde(default)]
    pub total_amount_due: f64,
    #[serde(default)]
    pub over_remittance: Form1601EqOverRemittance,

    #[serde(flatten)]
    pub lifecycle: SubmissionLifecycle,
}

/// `round(this, 2)` / `formatCurrency`: the value the official field holds.
fn cents(value: f64) -> f64 {
    parse_official_amount(&official_amount(value)).unwrap_or(0.0)
}

fn digits_only(value: &str) -> bool {
    value.bytes().all(|b| b.is_ascii_digit())
}

/// `(TIN1, TIN2, TIN3, branch)` from a stored TIN with or without dashes.
fn split_tin(tin: &str) -> (String, String, String, String) {
    let digits: String = tin.chars().filter(char::is_ascii_digit).collect();
    let part = |range: std::ops::Range<usize>| digits.get(range).unwrap_or("").to_string();
    let branch = digits.get(9..).unwrap_or("");
    (part(0..3), part(3..6), part(6..9), format!("{branch:0>5}"))
}

/// `round()` keeps at most 12 integer digits.
const MAX_AMOUNT: f64 = 1e12;

impl Form1601EqDraft {
    pub const FORM_CODE: &'static str = "1601EQ";

    pub fn new_from_profile(profile: &TaxpayerProfile, year: u16, quarter: u8) -> Self {
        let mut draft = Self {
            penalty: crate::forms::penalty_items::PenaltyState::for_profile(profile),
            id: None,
            tin: profile.tin.full(),
            taxable_year: year,
            quarter,
            is_amended: false,
            any_tax_withheld: None,
            number_of_attached_sheets: 0,
            rdo_code: profile.rdo_code.clone(),
            line_of_business: profile.line_of_business.clone(),
            taxpayer_name: profile.full_name.clone(),
            registered_address: profile.registered_address.clone(),
            zip_code: profile.zip_code.clone(),
            contact_number: profile.phone.clone(),
            category: None,
            email: profile.email.clone(),
            schedule: Vec::new(),
            total_other_tax_withheld: 0.0,
            total_taxes_withheld: 0.0,
            remittance_first_month: 0.0,
            remittance_second_month: 0.0,
            tax_remitted_previous: 0.0,
            over_remittance_previous_quarter: 0.0,
            total_remittances: 0.0,
            tax_still_due: 0.0,
            surcharge: 0.0,
            interest: 0.0,
            compromise: 0.0,
            total_penalties: 0.0,
            total_amount_due: 0.0,
            over_remittance: Form1601EqOverRemittance::None,
            lifecycle: SubmissionLifecycle::default(),
        };
        draft.recompute();
        draft
    }

    /// Item 4. "No" clears Part II's ATCs like `changeTaxWithheldNO`.
    pub fn set_any_tax_withheld(&mut self, withheld: bool) {
        self.any_tax_withheld = Some(withheld);
        if !withheld {
            self.schedule.clear();
        }
        self.recompute();
    }

    /// Item 11. Changing the category clears the ATCs like `changeCategory`.
    pub fn set_category(&mut self, category: Form1601EqCategory) {
        if self.category != Some(category) {
            self.schedule.clear();
        }
        self.category = Some(category);
        self.recompute();
    }

    /// Tick an ATC in the popup. The page lists rows in popup order, so the
    /// new row lands at its popup position.
    pub fn add_atc(&mut self, code: &str) -> Result<(), String> {
        if form_1601eq_atc_option(code).is_none() {
            return Err(format!(
                "{code} is not an ATC on the official 1601-EQ list."
            ));
        }
        if self.any_tax_withheld != Some(true) {
            return Err(
                "Selecting an ATC is not necessary when item no. 4 is set to ' NO '".to_string(),
            );
        }
        if self.category.is_none() {
            return Err("Please select an option for Item 11.".to_string());
        }
        if self.schedule.iter().any(|row| row.atc_code == code) {
            return Err(format!("{code} is already selected."));
        }
        self.schedule.push(Form1601EqAtcRow::new(code));
        self.recompute();
        Ok(())
    }

    /// Untick an ATC.
    pub fn remove_atc(&mut self, code: &str) {
        self.schedule.retain(|row| row.atc_code != code);
        self.recompute();
    }

    /// Rate text of a row as `getATCCode` copies it from the popup cell
    /// (`<td id='txtRate<i>'> 5.0</td>`), leading space included.
    fn row_rate_text(&self, row: &Form1601EqAtcRow) -> String {
        row.option()
            .map(|(_, option)| format!(" {}", option.rate_text_for_year(self.taxable_year)))
            .unwrap_or_default()
    }

    /// The official compute chain. Inputs are held at cents the way
    /// `round(this,2)` leaves them; each derived item is computed from the
    /// formatted values it reads, like the page.
    pub fn recompute(&mut self) {
        // getATCCode walks the popup list top to bottom.
        self.schedule
            .sort_by_key(|row| row.option().map_or(usize::MAX, |(index, _)| index));
        if !self.is_amended {
            self.tax_remitted_previous = 0.0;
        }
        let year = self.taxable_year;
        let mut total = 0.0;
        let mut other = 0.0;
        for (index, row) in self.schedule.iter_mut().enumerate() {
            row.tax_base = cents(row.tax_base);
            let rate = form_1601eq_atc_option(&row.atc_code)
                .map(|(_, option)| option.rate_for_year(year))
                .unwrap_or(0.0);
            // getRequiredWithheld: base * rate / 100.
            row.tax_withheld = cents(row.tax_base * rate / 100.0);
            total += row.tax_withheld;
            if index >= FORM_1601EQ_MAIN_ROWS {
                other += row.tax_withheld;
            }
        }
        self.total_taxes_withheld = cents(total);
        self.total_other_tax_withheld = cents(other);
        self.remittance_first_month = cents(self.remittance_first_month);
        self.remittance_second_month = cents(self.remittance_second_month);
        self.tax_remitted_previous = cents(self.tax_remitted_previous);
        self.over_remittance_previous_quarter = cents(self.over_remittance_previous_quarter);
        self.total_remittances = cents(
            self.remittance_first_month
                + self.remittance_second_month
                + self.tax_remitted_previous
                + self.over_remittance_previous_quarter,
        );
        self.tax_still_due = cents(self.total_taxes_withheld - self.total_remittances);
        // The penalty items file their override, else the computed value.
        Self::penalty_declaration().apply(self);
        self.surcharge = cents(self.surcharge);
        self.interest = cents(self.interest);
        self.compromise = cents(self.compromise);
        self.total_penalties = cents(self.surcharge + self.interest + self.compromise);
        self.total_amount_due = cents(self.tax_still_due + self.total_penalties);
        // computeOfTotalAmtDue clears the Item 30 boxes unless Item 30 < 0.
        if self.total_amount_due >= 0.0 {
            self.over_remittance = Form1601EqOverRemittance::None;
        }
    }

    /// Number of `AtcCode<i>` checkboxes in the popup when the return is
    /// submitted: none before Item 11 is answered; the list for the chosen
    /// category; and the private list once the popup has been opened to pick
    /// an ATC (see [`FORM_1601EQ_ATC_OPTIONS`]).
    fn popup_list_len(&self) -> usize {
        match self.category {
            None => 0,
            Some(_) if !self.schedule.is_empty() => FORM_1601EQ_ATC_OPTIONS.len(),
            Some(Form1601EqCategory::Private) => FORM_1601EQ_ATC_OPTIONS.len(),
            Some(Form1601EqCategory::Government) => FORM_1601EQ_GOVERNMENT_LIST_LEN,
        }
    }

    /// Rows the page has in Part II: always six on page 1, plus the "Other
    /// Selected ATC" rows.
    fn row_count(&self) -> usize {
        self.schedule.len().max(FORM_1601EQ_MAIN_ROWS)
    }

    /// The print sheet's field map: the upload map plus `derived:` keys
    /// that only shape the print.
    ///
    /// - `derived:tin_digits`: the later pages' header TIN, one 14-digit
    ///   comb (TIN then branch).
    /// - `derived:address_line1` / `derived:line:address_line2`: the
    ///   registered address over its 40-slot first line, broken at a
    ///   space, and the second line (printed whole when longer).
    pub fn to_print_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = self.to_bir_field_map();
        let tin_digits = super::print_map::digits_of(
            &fields,
            &[
                "frm1601EQ:txtTIN1",
                "frm1601EQ:txtTIN2",
                "frm1601EQ:txtTIN3",
                "frm1601EQ:txtBranchCode",
            ],
        );
        let address = self.registered_address.trim().to_uppercase();
        let (line1, line2) = super::print_map::split_print_line(&address, 40);
        for (key, value) in [
            ("tin_digits", tin_digits),
            ("address_line1", line1),
            ("line:address_line2", line2),
        ] {
            fields.insert(format!("derived:{key}"), value);
        }
        fields
    }

    /// The official field values `saveXMLsubmit` reads, keyed by element id.
    pub fn to_bir_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = BTreeMap::new();
        let mut put = |key: &str, value: String| {
            fields.insert(key.to_string(), value);
        };
        let p = |key: &str| format!("frm1601EQ:{key}");
        let flag = |on: bool| on.to_string();
        // capital() uppercases every text input except txtEmail.
        let text = |value: &str| value.trim().to_uppercase();

        put(&p("txtYear"), self.taxable_year.to_string());
        for quarter in 1..=4u8 {
            put(
                &p(&format!("optQuarter:{quarter}")),
                flag(self.quarter == quarter),
            );
        }
        put(&p("optAmend:Y"), flag(self.is_amended));
        put(&p("optAmend:N"), flag(!self.is_amended));
        put(
            &p("optWithheld:Y"),
            flag(self.any_tax_withheld == Some(true)),
        );
        put(
            &p("optWithheld:N"),
            flag(self.any_tax_withheld == Some(false)),
        );
        put(
            &p("txtNoSheets"),
            self.number_of_attached_sheets.to_string(),
        );

        let (tin1, tin2, tin3, branch) = split_tin(&self.tin);
        put(&p("txtTIN1"), tin1);
        put(&p("txtTIN2"), tin2);
        put(&p("txtTIN3"), tin3);
        put(&p("txtBranchCode"), branch);
        put(&p("txtRDOCode"), self.rdo_code.trim().to_string());
        put(&p("txtTaxpayerName"), text(&self.taxpayer_name));
        // loadBGData copies the profile's escape()d line of business as is
        // (no unescape, no capital() on this form).
        put(
            &p("txtLineBus"),
            crate::official_xml::js_escape(self.line_of_business.trim()),
        );
        // Address lines 1 and 2 are written back to back into one value.
        put(&p("txtAddress"), text(&self.registered_address));
        put(&p("txtAddress2"), String::new());
        put(&p("txtZipCode"), self.zip_code.trim().to_string());
        put(&p("txtTelNum"), self.contact_number.trim().to_string());
        put(
            &p("optCategory:P"),
            flag(self.category == Some(Form1601EqCategory::Private)),
        );
        put(
            &p("optCategory:G"),
            flag(self.category == Some(Form1601EqCategory::Government)),
        );
        put("txtEmail", self.email.trim().to_string());

        for index in 0..self.row_count() {
            let n = index + 1;
            match self.schedule.get(index) {
                Some(row) => {
                    put(&p(&format!("txtAtcCd{n}")), row.atc_code.clone());
                    put(&p(&format!("txtTaxBase{n}")), official_amount(row.tax_base));
                    put(&p(&format!("txtTaxRate{n}")), self.row_rate_text(row));
                    put(
                        &p(&format!("txtTaxbeWithHeld{n}")),
                        official_amount(row.tax_withheld),
                    );
                }
                None => {
                    put(&p(&format!("txtAtcCd{n}")), String::new());
                    put(&p(&format!("txtTaxBase{n}")), String::new());
                    put(&p(&format!("txtTaxRate{n}")), String::new());
                    put(&p(&format!("txtTaxbeWithHeld{n}")), "0.00".to_string());
                }
            }
        }
        // getATCCode fills the "Other Selected ATC" total; before any ATC is
        // picked the field is still blank.
        put(
            &p("txtTotalOtherTax"),
            if self.schedule.is_empty() {
                String::new()
            } else {
                official_amount(self.total_other_tax_withheld)
            },
        );

        put(&p("txtTax19"), official_amount(self.total_taxes_withheld));
        put(&p("txtTax20"), official_amount(self.remittance_first_month));
        put(
            &p("txtTax21"),
            official_amount(self.remittance_second_month),
        );
        put(&p("txtTax22"), official_amount(self.tax_remitted_previous));
        put(
            &p("txtTax23"),
            official_amount(self.over_remittance_previous_quarter),
        );
        put(&p("txtTax24"), official_amount(self.total_remittances));
        put(&p("txtTax25"), official_amount(self.tax_still_due));
        put(&p("txtTax26"), official_amount(self.surcharge));
        put(&p("txtTax27"), official_amount(self.interest));
        put(&p("txtTax28"), official_amount(self.compromise));
        put(&p("txtTax29"), official_amount(self.total_penalties));
        put(&p("txtTax30"), official_amount(self.total_amount_due));
        put(
            &p("ifRefund"),
            flag(self.over_remittance == Form1601EqOverRemittance::Refund),
        );
        put(
            &p("ifIssueCert"),
            flag(self.over_remittance == Form1601EqOverRemittance::TaxCreditCertificate),
        );
        put(
            &p("ifCarriedOver"),
            flag(self.over_remittance == Form1601EqOverRemittance::CarriedOver),
        );

        let ticked: Vec<usize> = self
            .schedule
            .iter()
            .filter_map(|row| row.option().map(|(index, _)| index))
            .collect();
        for index in 0..self.popup_list_len() {
            put(
                &format!("AtcCode{}", index + 1),
                flag(ticked.contains(&index)),
            );
        }
        // validateForm records the Part II table size (header + six rows).
        put("hPartIITableSize", "7".to_string());
        // 1601-EQ never calls getDrives(), so the export drive list is empty.
        put("driveSelectTPExport", String::new());
        fields
    }

    /// The generated official layout plus the controls the page draws at run
    /// time for this return (see the module docs).
    pub fn official_layout(&self) -> Result<OfficialLayout, crate::official_xml::OfficialXmlError> {
        let mut layout = crate::official_xml::layout(FORM_1601EQ_FORM_ID)?.clone();
        let position = |layout: &OfficialLayout, key: &str| {
            layout.entries.iter().position(|entry| match entry {
                Entry::Bool { key: k, .. } | Entry::Value { key: k, .. } => k == key,
            })
        };
        let missing =
            |key: &str| crate::official_xml::OfficialXmlError::UnknownKey(key.to_string());
        let email = position(&layout, "txtEmail").ok_or_else(|| missing("txtEmail"))?;
        let after = match &layout.entries[email] {
            Entry::Bool { after, .. } | Entry::Value { after, .. } => after.clone(),
        };
        let value = |key: String| Entry::Value {
            parts: vec![Part::Source {
                source: key.clone(),
                codec: Codec::Raw,
                uppercase: false,
            }],
            key,
            default: String::new(),
            number: None,
            after: after.clone(),
        };
        let row_entries = |rows: std::ops::Range<usize>| {
            rows.flat_map(|n| {
                [
                    format!("frm1601EQ:txtAtcCd{n}"),
                    format!("frm1601EQ:txtTaxBase{n}"),
                    format!("frm1601EQ:txtTaxRate{n}"),
                    format!("frm1601EQ:txtTaxbeWithHeld{n}"),
                ]
            })
            .map(value)
            .collect::<Vec<_>>()
        };

        let table_size =
            position(&layout, "hPartIITableSize").ok_or_else(|| missing("hPartIITableSize"))?;
        let other = row_entries(FORM_1601EQ_MAIN_ROWS + 1..self.row_count() + 1);
        layout.entries.splice(table_size + 1..table_size + 1, other);
        let checkboxes: Vec<Entry> = (1..=self.popup_list_len())
            .map(|n| Entry::Bool {
                key: format!("AtcCode{n}"),
                default: "false".to_string(),
                after: after.clone(),
            })
            .collect();
        layout.entries.splice(table_size..table_size, checkboxes);
        Ok(layout)
    }

    /// The exact official submit plaintext.
    pub fn to_bir_xml_payload(&self) -> Result<String, Vec<(String, String)>> {
        self.official_payload()
    }

    /// `validateForm` against a given "today" (Items 1 and 2 depend on it).
    pub fn validate_on(&self, today: chrono::NaiveDate) -> Vec<(String, String)> {
        use chrono::Datelike;
        let mut errors = Vec::new();
        let mut err = |field: &str, message: &str| {
            errors.push((field.to_string(), message.to_string()));
        };
        let current_year = u16::try_from(today.year()).unwrap_or(u16::MAX);
        // JavaScript getMonth(): 0 = January.
        let month0 = today.month0();

        // Item 1
        let year = self.taxable_year;
        let year_ok = if year == 0 {
            err("taxable_year", "Please enter a valid year on Item 1.");
            false
        } else if year > current_year {
            err(
                "taxable_year",
                "Invalid entry on Item 1. Entry should not be a future Date.",
            );
            false
        } else if year < 2018 {
            err(
                "taxable_year",
                "Invalid entry on Item 1. Entry should not be a previous year from 2018.",
            );
            false
        } else {
            true
        };

        // Item 2: the quarter must have ended.
        let past_year = year < current_year;
        match self.quarter {
            1 if year_ok && !past_year && month0 < 3 => err(
                "quarter",
                "Unable to select first Quarter due to the current date. Payment should be made after the Quarter",
            ),
            2 if year_ok && !past_year && month0 < 6 => err(
                "quarter",
                "Unable to select second Quarter due to the current date. Payment should be made after the Quarter",
            ),
            3 if year_ok && !past_year && month0 < 9 => err(
                "quarter",
                "Unable to select third Quarter due to the current date. Payment should be made after the Quarter",
            ),
            4 if year_ok && !past_year => err(
                "quarter",
                "Unable to select fourth Quarter due to the current date. Payment should be made after the Quarter",
            ),
            1..=4 => {}
            _ => err("quarter", "Please select Quarter on Item 2"),
        }

        if self.any_tax_withheld.is_none() {
            err("any_tax_withheld", "Please select an option for Item 4.");
        }
        if self.category.is_none() {
            err("category", "Please select an option for Item 11.");
        }
        if self.number_of_attached_sheets > 99 {
            err(
                "number_of_attached_sheets",
                "Item 5 holds at most two digits.",
            );
        }

        // Part I. The page only checks for blanks (these fields come from the
        // registration profile); the typing limits are enforced as well.
        let (tin1, tin2, tin3, _) = split_tin(&self.tin);
        let tin_digits: String = self.tin.chars().filter(char::is_ascii_digit).collect();
        if tin1.len() != 3
            || tin2.len() != 3
            || tin3.len() != 3
            || tin_digits.len() > 14
            || !self.tin.chars().all(|c| c.is_ascii_digit() || c == '-')
        {
            err("tin", "Please enter a valid TIN number on Item 6.");
        } else if !crate::validation::relaxed_dev_mode()
            && crate::validation::official_tin_check_code(&format!("{tin1}{tin2}{tin3}")) != 0
        {
            err("tin", crate::validation::OFFICIAL_INVALID_TIN_MESSAGE);
        }
        if !crate::validation::rdo_code_is_official_option(self.rdo_code.trim()) {
            err("rdo_code", "Please enter a valid RDO Code on Item 7.");
        }
        let name = self.taxpayer_name.trim();
        if name.is_empty() || name.chars().count() > 50 {
            err(
                "taxpayer_name",
                "Please enter a valid Taxpayer Name on Item 8.",
            );
        }
        let phone = self.contact_number.trim();
        if phone.is_empty() || phone.len() > 20 || !digits_only(phone) {
            err(
                "contact_number",
                "Please enter a valid Telephone Number on Item 10.",
            );
        }
        let address = self.registered_address.trim();
        // Two address lines of 150 characters each.
        if address.is_empty() || address.chars().count() > 300 {
            err(
                "registered_address",
                "Please enter Taxpayer's Registered Address on Item 9.",
            );
        }
        let zip = self.zip_code.trim();
        if zip.is_empty() || zip.len() > 12 || !digits_only(zip) {
            err("zip_code", "Please enter Taxpayer's Zip Code on Item 9A.");
        }
        let email = self.email.trim();
        if email.is_empty() || !email.contains('@') || email.contains(char::is_whitespace) {
            err("email", "Please enter valid Email Address on Item 12.");
        }
        if self.line_of_business.trim().chars().count() > 150 {
            err(
                "line_of_business",
                "Line of business holds at most 150 characters.",
            );
        }

        // Part II ATCs.
        match self.any_tax_withheld {
            Some(true) if self.schedule.is_empty() => err(
                "schedule",
                "Please fill up Part II Computation of Tax if item 4 is set to Yes.",
            ),
            Some(false) if !self.schedule.is_empty() => err(
                "schedule",
                "Selecting an ATC is not necessary when item no. 4 is set to ' NO '",
            ),
            _ => {}
        }
        for (index, row) in self.schedule.iter().enumerate() {
            if row.option().is_none() {
                err(
                    &format!("schedule[{index}].atc_code"),
                    &format!(
                        "{} is not an ATC on the official 1601-EQ list.",
                        row.atc_code
                    ),
                );
                continue;
            }
            if self.schedule[..index]
                .iter()
                .any(|other| other.atc_code == row.atc_code)
            {
                err(
                    &format!("schedule[{index}].atc_code"),
                    &format!("{} is already selected.", row.atc_code),
                );
            }
            if row.tax_base <= 0.0
                || row.tax_base >= MAX_AMOUNT
                || !has_cent_precision(row.tax_base)
            {
                err(
                    &format!("schedule[{index}].tax_base"),
                    &format!(
                        "Please enter a valid value for tax base for {}.",
                        row.atc_code
                    ),
                );
            }
        }

        // Typed amounts: numbersonly (Items 20–23) allows no sign; the
        // penalties have no key filter, but a negative penalty is never valid.
        for (field, value) in [
            ("remittance_first_month", self.remittance_first_month),
            ("remittance_second_month", self.remittance_second_month),
            ("tax_remitted_previous", self.tax_remitted_previous),
            (
                "over_remittance_previous_quarter",
                self.over_remittance_previous_quarter,
            ),
            ("surcharge", self.surcharge),
            ("interest", self.interest),
            ("compromise", self.compromise),
        ] {
            if value < 0.0 || value >= MAX_AMOUNT || !has_cent_precision(value) {
                err(field, "Enter a non-negative amount in pesos and centavos.");
            }
        }
        if !self.is_amended && self.tax_remitted_previous != 0.0 {
            err(
                "tax_remitted_previous",
                "Item 22 applies only to an amended return.",
            );
        }

        if self.total_amount_due < 0.0 && self.over_remittance == Form1601EqOverRemittance::None {
            err(
                "over_remittance",
                "Please select an Overpayment option in Part II Item 30.",
            );
        }
        if self.total_amount_due >= 0.0 && self.over_remittance != Form1601EqOverRemittance::None {
            err(
                "over_remittance",
                "Item 30 boxes apply only to an over-remittance.",
            );
        }

        // Derived items must be what the official compute chain produces.
        let mut expected = self.clone();
        expected.recompute();
        if expected.total_amount_due != self.total_amount_due
            || expected.total_taxes_withheld != self.total_taxes_withheld
            || expected.total_other_tax_withheld != self.total_other_tax_withheld
            || expected.schedule != self.schedule
        {
            err(
                "total_amount_due",
                "Totals are out of date. Recompute the return.",
            );
        }

        errors
    }
}

impl FormValidator for Form1601EqDraft {
    /// `validateForm` in order, with its alert texts, plus the input limits
    /// the official page enforces while typing.
    fn validate(&self) -> Vec<(String, String)> {
        self.validate_on(chrono::Local::now().date_naive())
    }
}

impl Form1601EqDraft {
    /// Items 26-28 on the shared penalty model ([`super::penalty_items`]).
    pub fn penalty_declaration() -> PenaltyItems<Self> {
        super::penalty_items::queueable_penalty_items!(
            Self,
            schedule: Withholding,
            items: [
                surcharge: "26 Surcharge",
                interest: "27 Interest",
                compromise: "28 Compromise",
            ],
            controls: [
                "frm1601EQ:txtTax26",
                "frm1601EQ:txtTax27",
                "frm1601EQ:txtTax28",
            ],
            state: penalty,
            basic_tax_due: "Item 25", |d| d.tax_still_due,
        )
    }
}

impl QueueableForm for Form1601EqDraft {
    fn print_field_map(&self) -> BTreeMap<String, String> {
        self.to_print_field_map()
    }
    fn penalty_items() -> Vec<PenaltyItems<Self>> {
        vec![Self::penalty_declaration()]
    }
    const FORM_CODE: &'static str = "1601EQ";
    /// Official `formType` (`1601EQ`).
    const FORM_TYPE: &'static str = "1601EQ";
    const LAYOUT_ID: &'static str = FORM_1601EQ_FORM_ID;

    /// Total Amount Still Due.
    fn amount_payable(&self) -> Option<f64> {
        Some(self.total_amount_due)
    }

    fn lifecycle(&self) -> &SubmissionLifecycle {
        &self.lifecycle
    }
    fn lifecycle_mut(&mut self) -> &mut SubmissionLifecycle {
        &mut self.lifecycle
    }

    /// Item "Amended Return?" = Yes, as the editor sets it.
    fn mark_amended(&mut self) -> bool {
        self.is_amended = true;
        self.compute();
        true
    }
    fn amended_return_answer(&self) -> Option<bool> {
        Some(self.is_amended)
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
    fn filing_period(&self) -> FilingPeriod {
        FilingPeriod::Quarterly(self.quarter)
    }
    /// `txtYear + "Q" + n`, as in `createXMLFileName`.
    fn period_code(&self) -> String {
        format!("{}Q{}", self.taxable_year, self.quarter)
    }
    fn parse_period_code(code: &str) -> Option<(u16, FilingPeriod)> {
        if code.len() != 6 || code.get(4..5)? != "Q" {
            return None;
        }
        let year: u16 = code.get(..4)?.parse().ok()?;
        let quarter: u8 = code.get(5..)?.parse().ok()?;
        (1..=4)
            .contains(&quarter)
            .then_some((year, FilingPeriod::Quarterly(quarter)))
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
        self.to_bir_field_map()
    }
    /// The generic writer over [`Self::official_layout`], which carries the
    /// controls the page draws at run time.
    fn official_payload(&self) -> Result<String, Vec<(String, String)>> {
        let errors = QueueableForm::validate(self);
        if !errors.is_empty() {
            return Err(errors);
        }
        let layout = self
            .official_layout()
            .map_err(|error| vec![("xml".to_string(), error.to_string())])?;
        crate::official_xml::write(&layout, &self.field_map())
            .map_err(|error| vec![("xml".to_string(), error.to_string())])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 10).unwrap()
    }

    pub(crate) fn sample() -> Form1601EqDraft {
        let mut draft = Form1601EqDraft {
            penalty: crate::forms::penalty_items::PenaltyState::new(None),
            id: None,
            tin: "12345678800000".to_string(),
            taxable_year: 2025,
            quarter: 1,
            is_amended: false,
            any_tax_withheld: None,
            number_of_attached_sheets: 0,
            rdo_code: "039".to_string(),
            line_of_business: "Consulting".to_string(),
            taxpayer_name: "Sample Taxpayer Inc".to_string(),
            registered_address: "123 Sample St Quezon City".to_string(),
            zip_code: "1100".to_string(),
            contact_number: "0281234567".to_string(),
            category: None,
            email: "sample.taxpayer@example.com".to_string(),
            schedule: Vec::new(),
            total_other_tax_withheld: 0.0,
            total_taxes_withheld: 0.0,
            remittance_first_month: 0.0,
            remittance_second_month: 0.0,
            tax_remitted_previous: 0.0,
            over_remittance_previous_quarter: 0.0,
            total_remittances: 0.0,
            tax_still_due: 0.0,
            surcharge: 0.0,
            interest: 0.0,
            compromise: 0.0,
            total_penalties: 0.0,
            total_amount_due: 0.0,
            over_remittance: Form1601EqOverRemittance::None,
            lifecycle: SubmissionLifecycle::default(),
        };
        draft.set_any_tax_withheld(true);
        draft.set_category(Form1601EqCategory::Private);
        // Ticked out of order; the page lists them in popup order.
        draft.add_atc("WI100").unwrap();
        draft.add_atc("WI010").unwrap();
        draft.schedule[0].tax_base = 123_456.789;
        draft.schedule[1].tax_base = 1_000.005;
        draft.remittance_first_month = 100.0;
        draft.surcharge = 10.0;
        draft.keep_entered_penalties();
        draft.recompute();
        draft
    }

    fn messages(draft: &Form1601EqDraft) -> Vec<String> {
        draft
            .validate_on(today())
            .into_iter()
            .map(|(_, message)| message)
            .collect()
    }

    #[test]
    fn compute_chain_matches_the_official_page() {
        // Values the official page shows for the same entries (runtime.js).
        let draft = sample();
        assert_eq!(draft.schedule[0].atc_code, "WI010");
        assert_eq!(draft.schedule[0].tax_base, 123_456.79);
        assert_eq!(draft.schedule[0].tax_withheld, 6_172.84);
        assert_eq!(draft.schedule[1].atc_code, "WI100");
        assert_eq!(draft.schedule[1].tax_base, 1_000.01);
        assert_eq!(draft.schedule[1].tax_withheld, 50.0);
        assert_eq!(draft.total_taxes_withheld, 6_222.84);
        assert_eq!(draft.total_remittances, 100.0);
        assert_eq!(draft.tax_still_due, 6_122.84);
        assert_eq!(draft.total_penalties, 10.0);
        assert_eq!(draft.total_amount_due, 6_132.84);
        assert!(messages(&draft).is_empty(), "{:?}", messages(&draft));
    }

    #[test]
    fn other_selected_atcs_beyond_six() {
        // runtime.js with eight ATCs: rows 7–8 go to "Other Selected ATC".
        let mut draft = sample();
        draft.schedule.clear();
        for code in [
            "WI010", "WI011", "WI020", "WI021", "WI030", "WI031", "WI040", "WI041",
        ] {
            draft.add_atc(code).unwrap();
        }
        for (row, base) in draft.schedule.iter_mut().zip([
            123_456.789,
            1_000.005,
            3.0,
            1.0,
            1.0,
            1.0,
            500.0,
            777.775,
        ]) {
            row.tax_base = base;
        }
        draft.recompute();
        assert_eq!(draft.schedule[7].tax_base, 777.78);
        assert_eq!(draft.schedule[7].tax_withheld, 77.78);
        assert_eq!(draft.total_other_tax_withheld, 102.78);
        let fields = draft.to_bir_field_map();
        assert_eq!(fields["frm1601EQ:txtTaxRate2"], " 10.0");
        assert_eq!(fields["frm1601EQ:txtTotalOtherTax"], "102.78");
        assert_eq!(fields["AtcCode8"], "true");
        let payload = draft.official_payload().unwrap();
        let other = payload.find("frm1601EQ:txtAtcCd7=WI040").unwrap();
        assert!(payload.find("<div>hPartIITableSize=7").unwrap() < other);
        assert!(payload.find("<div>AtcCode111=false").unwrap() < other);
    }

    #[test]
    fn field_map_uses_official_formats() {
        let draft = sample();
        let fields = draft.to_bir_field_map();
        assert_eq!(fields["frm1601EQ:txtAtcCd1"], "WI010");
        assert_eq!(fields["frm1601EQ:txtTaxBase1"], "123,456.79");
        assert_eq!(fields["frm1601EQ:txtTaxRate1"], " 5.0");
        assert_eq!(fields["frm1601EQ:txtTaxbeWithHeld1"], "6,172.84");
        assert_eq!(fields["frm1601EQ:txtAtcCd3"], "");
        assert_eq!(fields["frm1601EQ:txtTaxbeWithHeld3"], "0.00");
        assert_eq!(fields["frm1601EQ:txtTotalOtherTax"], "0.00");
        assert_eq!(fields["frm1601EQ:txtTaxpayerName"], "SAMPLE TAXPAYER INC");
        assert_eq!(fields["frm1601EQ:txtBranchCode"], "00000");
        assert_eq!(fields["AtcCode1"], "true");
        assert_eq!(fields["AtcCode19"], "true");
        assert_eq!(fields["AtcCode2"], "false");
        assert_eq!(fields["txtEmail"], "sample.taxpayer@example.com");
        assert_eq!(
            draft.submission_filename(),
            "12345678800000-1601EQ-2025Q1#sample.taxpayer@example.com#.xml"
        );
        // Every key is a control of the run-time layout.
        let layout = draft.official_layout().unwrap();
        let keys = layout.keys();
        for key in fields.keys() {
            assert!(keys.contains(key.as_str()), "{key}");
        }
    }

    #[test]
    fn popup_list_and_rates_follow_the_page() {
        let mut draft = sample();
        draft.schedule.clear();
        draft.recompute();
        // Before any ATC, the list is the one Item 11 drew.
        assert_eq!(draft.popup_list_len(), 111);
        draft.set_category(Form1601EqCategory::Government);
        assert_eq!(draft.popup_list_len(), 96);
        draft.add_atc("WC710").unwrap();
        assert_eq!(draft.popup_list_len(), 111);
        assert_eq!(draft.to_bir_field_map()["frm1601EQ:txtTaxRate1"], " 15.0");
        // changeATCRate: 2018 returns keep the old rates.
        draft.taxable_year = 2018;
        assert_eq!(draft.to_bir_field_map()["frm1601EQ:txtTaxRate1"], " 20.0");
        assert!(draft.add_atc("XX999").is_err());
        assert!(draft.add_atc("WC710").is_err());
    }

    #[test]
    fn period_codes_round_trip() {
        let draft = sample();
        assert_eq!(draft.period_code(), "2025Q1");
        assert_eq!(
            Form1601EqDraft::parse_period_code("2025Q1"),
            Some((2025, FilingPeriod::Quarterly(1)))
        );
        assert_eq!(Form1601EqDraft::parse_period_code("2025Q5"), None);
        assert_eq!(Form1601EqDraft::parse_period_code("122025Q1"), None);
        assert_eq!(Form1601EqDraft::parse_period_code("2025"), None);
    }

    #[test]
    fn official_negative_cases_are_rejected_with_their_alerts() {
        let check = |mutate: &dyn Fn(&mut Form1601EqDraft), expected: &str| {
            let mut draft = sample();
            mutate(&mut draft);
            let found = messages(&draft);
            assert!(
                found.iter().any(|m| m == expected),
                "{expected:?} not in {found:?}"
            );
        };
        check(
            &|d| d.taxable_year = 0,
            "Please enter a valid year on Item 1.",
        );
        check(
            &|d| d.taxable_year = 2027,
            "Invalid entry on Item 1. Entry should not be a future Date.",
        );
        check(
            &|d| d.taxable_year = 2017,
            "Invalid entry on Item 1. Entry should not be a previous year from 2018.",
        );
        check(
            &|d| {
                d.taxable_year = 2026;
                d.quarter = 4;
            },
            "Unable to select fourth Quarter due to the current date. Payment should be made after the Quarter",
        );
        check(&|d| d.quarter = 0, "Please select Quarter on Item 2");
        check(
            &|d| d.any_tax_withheld = None,
            "Please select an option for Item 4.",
        );
        check(
            &|d| d.category = None,
            "Please select an option for Item 11.",
        );
        check(
            &|d| d.tin = "12345".into(),
            "Please enter a valid TIN number on Item 6.",
        );
        check(
            &|d| d.tin = "12345678900000".into(),
            "You have entered an incorrect TIN",
        );
        check(
            &|d| d.rdo_code = String::new(),
            "Please enter a valid RDO Code on Item 7.",
        );
        check(
            &|d| d.taxpayer_name.clear(),
            "Please enter a valid Taxpayer Name on Item 8.",
        );
        check(
            &|d| d.contact_number = "02-123".into(),
            "Please enter a valid Telephone Number on Item 10.",
        );
        check(
            &|d| d.registered_address.clear(),
            "Please enter Taxpayer's Registered Address on Item 9.",
        );
        check(
            &|d| d.zip_code.clear(),
            "Please enter Taxpayer's Zip Code on Item 9A.",
        );
        check(
            &|d| d.email.clear(),
            "Please enter valid Email Address on Item 12.",
        );
        check(
            &|d| {
                d.schedule.clear();
                d.recompute();
            },
            "Please fill up Part II Computation of Tax if item 4 is set to Yes.",
        );
        check(
            &|d| {
                d.schedule[0].tax_base = 0.0;
                d.recompute();
            },
            "Please enter a valid value for tax base for WI010.",
        );
        check(
            &|d| {
                d.remittance_first_month = 100_000.0;
                d.recompute();
            },
            "Please select an Overpayment option in Part II Item 30.",
        );
        check(
            &|d| d.any_tax_withheld = Some(false),
            "Selecting an ATC is not necessary when item no. 4 is set to ' NO '",
        );
    }

    #[test]
    fn current_year_quarters_need_the_quarter_to_end() {
        let mut draft = sample();
        draft.taxable_year = 2026;
        draft.quarter = 3;
        let early = NaiveDate::from_ymd_opt(2026, 9, 30).unwrap();
        assert!(
            draft
                .validate_on(early)
                .iter()
                .any(|(_, m)| m.starts_with("Unable to select third Quarter"))
        );
        assert!(draft.validate_on(today()).is_empty());
    }

    #[test]
    fn withheld_no_clears_atcs_and_still_files() {
        let mut draft = sample();
        draft.set_any_tax_withheld(false);
        assert!(draft.schedule.is_empty());
        draft.recompute();
        assert_eq!(draft.total_amount_due, -90.0);
        assert!(
            messages(&draft)
                .iter()
                .any(|m| m == "Please select an Overpayment option in Part II Item 30.")
        );
        draft.over_remittance = Form1601EqOverRemittance::CarriedOver;
        assert!(messages(&draft).is_empty(), "{:?}", messages(&draft));
        let fields = draft.to_bir_field_map();
        assert_eq!(fields["frm1601EQ:txtTotalOtherTax"], "");
        assert_eq!(fields["frm1601EQ:ifCarriedOver"], "true");
        assert_eq!(fields["frm1601EQ:txtTax30"], "-90.00");
    }

    #[test]
    fn queue_and_revalidate_through_the_generic_path() {
        let mut draft = sample();
        draft
            .queue(crate::filing_queue::QueueAuthSource::Gui)
            .unwrap();
        assert!(draft.revalidate_queued_before_submission().is_ok());
        draft.surcharge = 99.0;
        draft.keep_entered_penalties();
        assert!(draft.revalidate_queued_before_submission().is_err());
    }

    /// P5-1: the earlier float-tolerance check refused two-decimal
    /// amounts above about 2,000,000 (`8_816_764.47 * 100` lands 1.2e-7
    /// off a whole number); the shared `official_inputs` check accepts
    /// them again, and a three-decimal amount is still refused.
    #[test]
    fn large_two_decimal_amounts_pass_cent_precision() {
        let mut draft = sample();
        draft.surcharge = 8_816_764.47;
        assert!(
            <Form1601EqDraft as FormValidator>::validate(&draft)
                .iter()
                .all(|(field, _)| field != "surcharge"),
            "a large two-decimal amount must be accepted"
        );
        draft.surcharge = 1.005;
        assert!(
            <Form1601EqDraft as FormValidator>::validate(&draft)
                .iter()
                .any(|(field, _)| field == "surcharge"),
            "a three-decimal amount must still be refused"
        );
    }
}
