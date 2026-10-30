//! BIR Form 1601-FQ — Quarterly Remittance Return of Final Income Taxes
//! Withheld, January 2018 (ENCS).
//!
//! Ported from the official `BIR-Form1601FQ.hta` (eBIRForms 7.9.6.2.1): the
//! ATC popup (`changedrpATCList` / `getATCCode`), Schedule 1 (treaty rates,
//! `getATCdrpTaxRate` / `getReqWithheldCompute`), the compute chain
//! (`computeofTotalWithheldTax`), `validate()` with its exact alert texts,
//! and the uploaded file `saveEncryptedProfile` writes (every `frmMain`
//! control in DOM order).
//!
//! The generated layout (`data/official-xml/1601fq-v2018.json`) already has
//! Items 14–19, which `populateAtcPart2()` draws at load. What the page adds
//! later, [`Form1601FqDraft::official_layout`] adds where the DOM has it: the
//! popup's ATC checkboxes (`AtcCd1..n`, plus the "N/A" special-law row
//! `enableSelTreaty()` appends when Item 13 is "Yes") and the "Other Selected
//! ATC" rows 7+, right before `txtFinalFlag`.

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
pub const FORM_1601FQ_FORM_ID: &str = "1601fq-v2018";
/// Items 14–19 on page 1; further ATCs go to "Other Selected ATC".
pub const FORM_1601FQ_MAIN_ROWS: usize = 6;
/// Item number of the first ATC row.
pub const FORM_1601FQ_FIRST_ITEM: usize = 14;
/// Schedule 1 has five rows (`drpTreatyCode0..4`).
pub const FORM_1601FQ_SCHEDULE1_ROWS: usize = 5;

/// One entry of the official ATC popup (`atcCodes.xml`, form 1601FQ).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Form1601FqAtcOption {
    pub code: &'static str,
    pub description: &'static str,
    /// Rate text the popup shows, e.g. `"20.0"`.
    pub rate_text: &'static str,
}

impl Form1601FqAtcOption {
    pub fn rate(&self) -> f64 {
        self.rate_text.parse().unwrap_or(0.0)
    }
}

/// Popup list for Item 11 "Private" in official order (`atcCodes.xml`, category `P`).
pub const FORM_1601FQ_PRIVATE_ATCS: &[Form1601FqAtcOption] = &[
    Form1601FqAtcOption {
        code: "WI226",
        description: "SHARE OF NRAETB IN THE DISTRIBUTABLE NET INCOME AFTER TAX OF A PARTNERSHIP (EXCEPT GENERAL PROFESSIONAL PARTNERSHIP) OF WHICH HE IS A PARTNER, OR SHARE IN THE NET INCOME AFTER TAX OF AN ASSOCIATION, JOINT ACCOUNT OR A JOINT VENTURE TAXABLE AS A CORPORATION OF WHICH HE IS A MEMBER OR A CO-VENTURER",
        rate_text: "20.0",
    },
    Form1601FqAtcOption {
        code: "WI240",
        description: "DISTRIBUTIVE SHARE OF INDIVIDUAL PARTNERS IN A TAXABLE PARTNERSHIP, ASSOCIATION, JOINT ACCOUNT OR JOINT VENTURE OR CONSORTIUM",
        rate_text: "10.0",
    },
    Form1601FqAtcOption {
        code: "WI250",
        description: "ALL KINDS OF ROYALTY PAYMENTS TO CITIZENS, RESIDENTS ALIENS AND NRAETB (OTHER THAN WI380 AND WI341), DOMESTIC AND RESIDENT FOREIGN CORPORATIONS",
        rate_text: "20.0",
    },
    Form1601FqAtcOption {
        code: "WI260",
        description: "ON PRIZES EXCEEDING P10,000 AND OTHER WINNINGS PAID TO INDIVIDUALS",
        rate_text: "20.0",
    },
    Form1601FqAtcOption {
        code: "WI310",
        description: "ON PAYMENTS TO OIL EXPLORATION SERVICE CONTRACTORS/SUB-CONTRACTORS",
        rate_text: "8.0",
    },
    Form1601FqAtcOption {
        code: "WI330",
        description: "PAYMENTS TO NON-RESIDENT ALIEN NOT ENGAGE IN TRADE OR BUSINESS WITHIN THE PHILIPPINES (NRANETB) EXCEPT ON SALE OF SHARES IN DOMESTIC CORPORATION AND REAL PROPERTY",
        rate_text: "25.0",
    },
    Form1601FqAtcOption {
        code: "WI340",
        description: "ON PAYMENTS TO NON-RESIDENT INDIVIDUAL/FOREIGN CORPORATE CINEMATOGRAPHIC FILM OWNERS, LESSORS OR DISTRIBUTORS",
        rate_text: "25.0",
    },
    Form1601FqAtcOption {
        code: "WI341",
        description: "ROYALTIES PAID TO NRAETB ON CINEMATOGRAPHIC FILMS AND SIMILAR WORKS",
        rate_text: "25.0",
    },
    Form1601FqAtcOption {
        code: "WI350",
        description: "FINAL TAX ON INTEREST OR OTHER PAYMENTS UPON TAX-FREE COVENANT BONDS, MORTGAGES, DEEDS OF TRUST OR OTHER OBLIGATIONS UNDER SEC. 57C OF THE NATIONAL INTERNAL REVENUE CODE OF 1997, AS AMENDED",
        rate_text: "30.0",
    },
    Form1601FqAtcOption {
        code: "WI380",
        description: "ROYALTIES PAID TO CITIZENS, RESIDENT ALIENS AND NRAETB ON BOOKS, OTHER LITERARY WORKS AND MUSICAL COMPOSITIONS",
        rate_text: "10.0",
    },
    Form1601FqAtcOption {
        code: "WI410",
        description: "INFORMERS CASH REWARD TO INDIVIDUALS/JURIDICAL PERSONS",
        rate_text: "10.0",
    },
    Form1601FqAtcOption {
        code: "WI700",
        description: "CASH OR PROPERTY DIVIDEND PAID BY A REAL ESTATE INVESTMENT TRUST (REIT)",
        rate_text: "10.0",
    },
    Form1601FqAtcOption {
        code: "WC180",
        description: "INTEREST ON FOREIGN LOANS PAYABLE TO NON-RESIDENT FOREIGN CORPORATIONS (NRFCS)",
        rate_text: "20.0",
    },
    Form1601FqAtcOption {
        code: "WC190",
        description: "INTEREST AND OTHER INCOME PAYMENTS ON FOREIGN CURRENCY TRANSACTIONS/LOANS PAYABLE TO OFFSHORE BANKING UNITS (OBUS)",
        rate_text: "10.0",
    },
    Form1601FqAtcOption {
        code: "WC191",
        description: "INTEREST AND OTHER INCOME PAYMENTS ON FOREIGN CURRENCY TRANSACTIONS/LOANS PAYABLE TO FOREIGN CURRENCY DEPOSIT UNITS (FCDUS)",
        rate_text: "10.0",
    },
    Form1601FqAtcOption {
        code: "WC212",
        description: "CASH DIVIDEND PAYMENT BY DOMESTIC CORPORATION TO CITIZENS AND RESIDENT ALIENS/NRFCS",
        rate_text: "30.0",
    },
    Form1601FqAtcOption {
        code: "WC213",
        description: "PROPERTY DIVIDEND PAYMENT BY DOMESTIC CORPORATION TO CITIZENS AND RESIDENT ALIENS/NRFCS",
        rate_text: "30.0",
    },
    Form1601FqAtcOption {
        code: "WC222",
        description: "CASH DIVIDEND PAYMENT BY DOMESTIC CORPORATION TO NRFCS WHOSE COUNTRIES ALLOWED TAX DEEMED PAID CREDIT (SUBJECT TO TAX SPARING RULE)",
        rate_text: "15.0",
    },
    Form1601FqAtcOption {
        code: "WC223",
        description: "PROPERTY DIVIDEND PAYMENT BY DOMESTIC CORPORATION TO NRFCS WHOSE COUNTRIES ALLOWED TAX DEEMED PAID CREDIT (SUBJECT TO TAX SPARING RULE)",
        rate_text: "15.0",
    },
    Form1601FqAtcOption {
        code: "WC230",
        description: "ON OTHER PAYMENTS TO NRFCS",
        rate_text: "30.0",
    },
    Form1601FqAtcOption {
        code: "WC250",
        description: "ALL KINDS OF ROYALTY PAYMENTS TO CITIZENS, RESIDENTS ALIENS AND NRAETB (OTHER THAN WI380 AND WI341), DOMESTIC AND RESIDENT FOREIGN CORPORATIONS",
        rate_text: "20.0",
    },
    Form1601FqAtcOption {
        code: "WC280",
        description: "BRANCH PROFIT REMITTANCES BY ALL CORPORATIONS EXCEPT PEZA/SBMA/CDA REGISTERED",
        rate_text: "15.0",
    },
    Form1601FqAtcOption {
        code: "WC290",
        description: "ON THE GROSS RENTALS, LEASE AND CHARTER FEES DERIVED BY NON-RESIDENT OWNER OR LESSOR OF FOREIGN VESSELS",
        rate_text: "4.5",
    },
    Form1601FqAtcOption {
        code: "WC300",
        description: "ON THE GROSS RENTALS, CHARTERS AND OTHER FEES DERIVED BY NON-RESIDENT LESSOR OR AIRCRAFT, MACHINERIES AND EQUIPMENT",
        rate_text: "7.5",
    },
    Form1601FqAtcOption {
        code: "WC310",
        description: "ON PAYMENTS TO OIL EXPLORATION SERVICE CONTRACTORS/SUB-CONTRACTORS",
        rate_text: "8.0",
    },
    Form1601FqAtcOption {
        code: "WC340",
        description: "ON PAYMENTS TO NON-RESIDENT INDIVIDUAL/FOREIGN CORPORATE CINEMATOGRAPHIC FILM OWNERS, LESSORS OR DISTRIBUTORS",
        rate_text: "25.0",
    },
    Form1601FqAtcOption {
        code: "WC410",
        description: "INFORMERS CASH REWARD TO INDIVIDUALS/JURIDICAL PERSONS",
        rate_text: "10.0",
    },
    Form1601FqAtcOption {
        code: "WC700",
        description: "CASH OR PROPERTY DIVIDEND PAID BY A REAL ESTATE INVESTMENT TRUST (REIT)",
        rate_text: "10.0",
    },
    Form1601FqAtcOption {
        code: "WI202",
        description: "CASH DIVIDEND PAYMENT BY DOMESTIC CORPORATION TO CITIZENS AND RESIDENT ALIENS/NRFCS",
        rate_text: "10.0",
    },
    Form1601FqAtcOption {
        code: "WC212",
        description: "CASH DIVIDEND PAYMENT BY DOMESTIC CORPORATION TO CITIZENS AND RESIDENT ALIENS/NRFCS",
        rate_text: "25.0",
    },
    Form1601FqAtcOption {
        code: "WC213",
        description: "PROPERTY DIVIDEND PAYMENT BY DOMESTIC CORPORATION TO CITIZENS AND RESIDENT ALIENS/NRFCS",
        rate_text: "25.0",
    },
    Form1601FqAtcOption {
        code: "WC810",
        description: "FINAL TAX REPRESENTING FRANCHISE TAX ON PAYMENTS TO NRFC SUPPLIER OF PAGCOR RELATED TO ITS GAMING OPERATIONS",
        rate_text: "5.0",
    },
    Form1601FqAtcOption {
        code: "WI350",
        description: "FINAL TAX ON INTEREST OR OTHER PAYMENTS UPON TAX-FREE COVENANT BONDS, MORTGAGES, DEEDS OF TRUST OR OTHER OBLIGATIONS UNDER SEC. 57C OF THE NATIONAL INTERNAL REVENUE CODE OF 1997, AS AMENDED",
        rate_text: "20.0",
    },
    Form1601FqAtcOption {
        code: "WI740",
        description: "FINAL WT ON FOREIGN NATIONALS EMPLOYED BY POGO ENTITIES",
        rate_text: "25.0",
    },
    Form1601FqAtcOption {
        code: "WI750",
        description: "ON GROSS INCOME EARNED BY FOREIGN NATIONALS OR NON-FILIPINO CITIZENS, REGARDLESS OF THEIR RESIDENCY, WHO ARE EMPLOYED AND ASSIGNED IN THE PHILIPPINES BY OFFSHORE GAMING LICENSEE OR ITS ACCREDITED SERVICE PROVIDER.",
        rate_text: "25.0",
    },
    Form1601FqAtcOption {
        code: "WC230",
        description: "ON OTHER PAYMENTS TO NRFCS",
        rate_text: "25.0",
    },
    Form1601FqAtcOption {
        code: "WI202",
        description: "CASH DIVIDEND PAYMENT BY DOMESTIC CORPORATION TO CITIZENS AND RESIDENT ALIENS/NRFCS",
        rate_text: "10.0",
    },
    Form1601FqAtcOption {
        code: "WI203",
        description: "PROPERTY DIVIDEND PAYMENT BY DOMESTIC CORPORATION TO CITIZENS AND RESIDENT ALIENS/NRFCS",
        rate_text: "10.0",
    },
    Form1601FqAtcOption {
        code: "WI224",
        description: "CASH DIVIDEND PAYMENT BY DOMESTIC CORPORATION TO NON-RESIDENT ALIEN ENGAGE IN TRADE OR BUSINESS WITHIN THE PHILIPPINES (NRAETB)",
        rate_text: "20.0",
    },
    Form1601FqAtcOption {
        code: "WI225",
        description: "PROPERTY DIVIDEND PAYMENT BY DOMESTIC CORPORATION TO NRAETB",
        rate_text: "20.0",
    },
];

/// Popup list for Item 11 "Government" in official order (`atcCodes.xml`, category `G`).
pub const FORM_1601FQ_GOVERNMENT_ATCS: &[Form1601FqAtcOption] = &[
    Form1601FqAtcOption {
        code: "WI203",
        description: "PROPERTY DIVIDEND PAYMENT BY DOMESTIC CORPORATION TO CITIZENS AND RESIDENT ALIENS/NRFCS",
        rate_text: "10.0",
    },
    Form1601FqAtcOption {
        code: "WI224",
        description: "CASH DIVIDEND PAYMENT BY DOMESTIC CORPORATION TO NON-RESIDENT ALIEN ENGAGE IN TRADE OR BUSINESS WITHIN THE PHILIPPINES (NRAETB)",
        rate_text: "20.0",
    },
    Form1601FqAtcOption {
        code: "WI225",
        description: "PROPERTY DIVIDEND PAYMENT BY DOMESTIC CORPORATION TO NRAETB",
        rate_text: "20.0",
    },
    Form1601FqAtcOption {
        code: "WI226",
        description: "SHARE OF NRAETB IN THE DISTRIBUTABLE NET INCOME AFTER TAX OF A PARTNERSHIP (EXCEPT GENERAL PROFESSIONAL PARTNERSHIP) OF WHICH HE IS A PARTNER, OR SHARE IN THE NET INCOME AFTER TAX OF AN ASSOCIATION, JOINT ACCOUNT OR A JOINT VENTURE TAXABLE AS A CORPORATION OF WHICH HE IS A MEMBER OR A CO-VENTURER",
        rate_text: "20.0",
    },
    Form1601FqAtcOption {
        code: "WI240",
        description: "DISTRIBUTIVE SHARE OF INDIVIDUAL PARTNERS IN A TAXABLE PARTNERSHIP, ASSOCIATION, JOINT ACCOUNT OR JOINT VENTURE OR CONSORTIUM",
        rate_text: "10.0",
    },
    Form1601FqAtcOption {
        code: "WI250",
        description: "ALL KINDS OF ROYALTY PAYMENTS TO CITIZENS, RESIDENTS ALIENS AND NRAETB (OTHER THAN WI380 AND WI341), DOMESTIC AND RESIDENT FOREIGN CORPORATIONS",
        rate_text: "20.0",
    },
    Form1601FqAtcOption {
        code: "WI260",
        description: "ON PRIZES EXCEEDING P10,000 AND OTHER WINNINGS PAID TO INDIVIDUALS",
        rate_text: "20.0",
    },
    Form1601FqAtcOption {
        code: "WI310",
        description: "ON PAYMENTS TO OIL EXPLORATION SERVICE CONTRACTORS/SUB-CONTRACTORS",
        rate_text: "8.0",
    },
    Form1601FqAtcOption {
        code: "WI330",
        description: "PAYMENTS TO NON-RESIDENT ALIEN NOT ENGAGE IN TRADE OR BUSINESS WITHIN THE PHILIPPINES (NRANETB) EXCEPT ON SALE OF SHARES IN DOMESTIC CORPORATION AND REAL PROPERTY",
        rate_text: "25.0",
    },
    Form1601FqAtcOption {
        code: "WI340",
        description: "ON PAYMENTS TO NON-RESIDENT INDIVIDUAL/FOREIGN CORPORATE CINEMATOGRAPHIC FILM OWNERS, LESSORS OR DISTRIBUTORS",
        rate_text: "25.0",
    },
    Form1601FqAtcOption {
        code: "WI341",
        description: "ROYALTIES PAID TO NRAETB ON CINEMATOGRAPHIC FILMS AND SIMILAR WORKS",
        rate_text: "25.0",
    },
    Form1601FqAtcOption {
        code: "WI350",
        description: "FINAL TAX ON INTEREST OR OTHER PAYMENTS UPON TAX-FREE COVENANT BONDS, MORTGAGES, DEEDS OF TRUST OR OTHER OBLIGATIONS UNDER SEC. 57C OF THE NATIONAL INTERNAL REVENUE CODE OF 1997, AS AMENDED",
        rate_text: "30.0",
    },
    Form1601FqAtcOption {
        code: "WI380",
        description: "ROYALTIES PAID TO CITIZENS, RESIDENT ALIENS AND NRAETB ON BOOKS, OTHER LITERARY WORKS AND MUSICAL COMPOSITIONS",
        rate_text: "10.0",
    },
    Form1601FqAtcOption {
        code: "WI410",
        description: "INFORMERS CASH REWARD TO INDIVIDUALS/JURIDICAL PERSONS",
        rate_text: "10.0",
    },
    Form1601FqAtcOption {
        code: "WI700",
        description: "CASH OR PROPERTY DIVIDEND PAID BY A REAL ESTATE INVESTMENT TRUST (REIT)",
        rate_text: "10.0",
    },
    Form1601FqAtcOption {
        code: "WC180",
        description: "INTEREST ON FOREIGN LOANS PAYABLE TO NON-RESIDENT FOREIGN CORPORATIONS (NRFCS)",
        rate_text: "20.0",
    },
    Form1601FqAtcOption {
        code: "WC190",
        description: "INTEREST AND OTHER INCOME PAYMENTS ON FOREIGN CURRENCY TRANSACTIONS/LOANS PAYABLE TO OFFSHORE BANKING UNITS (OBUS)",
        rate_text: "10.0",
    },
    Form1601FqAtcOption {
        code: "WC191",
        description: "INTEREST AND OTHER INCOME PAYMENTS ON FOREIGN CURRENCY TRANSACTIONS/LOANS PAYABLE TO FOREIGN CURRENCY DEPOSIT UNITS (FCDUS)",
        rate_text: "10.0",
    },
    Form1601FqAtcOption {
        code: "WC212",
        description: "CASH DIVIDEND PAYMENT BY DOMESTIC CORPORATION TO CITIZENS AND RESIDENT ALIENS/NRFCS",
        rate_text: "30.0",
    },
    Form1601FqAtcOption {
        code: "WC213",
        description: "PROPERTY DIVIDEND PAYMENT BY DOMESTIC CORPORATION TO CITIZENS AND RESIDENT ALIENS/NRFCS",
        rate_text: "30.0",
    },
    Form1601FqAtcOption {
        code: "WC222",
        description: "CASH DIVIDEND PAYMENT BY DOMESTIC CORPORATION TO NRFCS WHOSE COUNTRIES ALLOWED TAX DEEMED PAID CREDIT (SUBJECT TO TAX SPARING RULE)",
        rate_text: "15.0",
    },
    Form1601FqAtcOption {
        code: "WC223",
        description: "PROPERTY DIVIDEND PAYMENT BY DOMESTIC CORPORATION TO NRFCS WHOSE COUNTRIES ALLOWED TAX DEEMED PAID CREDIT (SUBJECT TO TAX SPARING RULE)",
        rate_text: "15.0",
    },
    Form1601FqAtcOption {
        code: "WC230",
        description: "ON OTHER PAYMENTS TO NRFCS",
        rate_text: "30.0",
    },
    Form1601FqAtcOption {
        code: "WC250",
        description: "ALL KINDS OF ROYALTY PAYMENTS TO CITIZENS, RESIDENTS ALIENS AND NRAETB (OTHER THAN WI380 AND WI341), DOMESTIC AND RESIDENT FOREIGN CORPORATIONS",
        rate_text: "20.0",
    },
    Form1601FqAtcOption {
        code: "WC280",
        description: "BRANCH PROFIT REMITTANCES BY ALL CORPORATIONS EXCEPT PEZA/SBMA/CDA REGISTERED",
        rate_text: "15.0",
    },
    Form1601FqAtcOption {
        code: "WC290",
        description: "ON THE GROSS RENTALS, LEASE AND CHARTER FEES DERIVED BY NON-RESIDENT OWNER OR LESSOR OF FOREIGN VESSELS",
        rate_text: "4.5",
    },
    Form1601FqAtcOption {
        code: "WC300",
        description: "ON THE GROSS RENTALS, CHARTERS AND OTHER FEES DERIVED BY NON-RESIDENT LESSOR OR AIRCRAFT, MACHINERIES AND EQUIPMENT",
        rate_text: "7.5",
    },
    Form1601FqAtcOption {
        code: "WC310",
        description: "ON PAYMENTS TO OIL EXPLORATION SERVICE CONTRACTORS/SUB-CONTRACTORS",
        rate_text: "8.0",
    },
    Form1601FqAtcOption {
        code: "WC340",
        description: "ON PAYMENTS TO NON-RESIDENT INDIVIDUAL/FOREIGN CORPORATE CINEMATOGRAPHIC FILM OWNERS, LESSORS OR DISTRIBUTORS",
        rate_text: "25.0",
    },
    Form1601FqAtcOption {
        code: "WC410",
        description: "INFORMERS CASH REWARD TO INDIVIDUALS/JURIDICAL PERSONS",
        rate_text: "10.0",
    },
    Form1601FqAtcOption {
        code: "WC700",
        description: "CASH OR PROPERTY DIVIDEND PAID BY A REAL ESTATE INVESTMENT TRUST (REIT)",
        rate_text: "10.0",
    },
    Form1601FqAtcOption {
        code: "WI240",
        description: "DISTRIBUTIVE SHARE OF INDIVIDUAL PARTNERS IN A TAXABLE PARTNERSHIP, ASSOCIATION, JOINT ACCOUNT OR JOINT VENTURE OR CONSORTIUM",
        rate_text: "20.0",
    },
    Form1601FqAtcOption {
        code: "WI341",
        description: "ROYALTIES PAID TO NRAETB ON CINEMATOGRAPHIC FILMS AND SIMILAR WORKS",
        rate_text: "20.0",
    },
    Form1601FqAtcOption {
        code: "WC212",
        description: "CASH DIVIDEND PAYMENT BY DOMESTIC CORPORATION TO CITIZENS AND RESIDENT ALIENS/NRFCS",
        rate_text: "25.0",
    },
    Form1601FqAtcOption {
        code: "WC213",
        description: "PROPERTY DIVIDEND PAYMENT BY DOMESTIC CORPORATION TO CITIZENS AND RESIDENT ALIENS/NRFCS",
        rate_text: "25.0",
    },
    Form1601FqAtcOption {
        code: "WC230",
        description: "ON OTHER PAYMENTS TO NRFCS",
        rate_text: "25.0",
    },
    Form1601FqAtcOption {
        code: "WI240",
        description: "DISTRIBUTIVE SHARE OF INDIVIDUAL PARTNERS IN A TAXABLE PARTNERSHIP, ASSOCIATION, JOINT ACCOUNT OR JOINT VENTURE OR CONSORTIUM",
        rate_text: "20.0",
    },
    Form1601FqAtcOption {
        code: "WI341",
        description: "ROYALTIES PAID TO NRAETB ON CINEMATOGRAPHIC FILMS AND SIMILAR WORKS",
        rate_text: "20.0",
    },
    Form1601FqAtcOption {
        code: "WI350",
        description: "FINAL TAX ON INTEREST OR OTHER PAYMENTS UPON TAX-FREE COVENANT BONDS, MORTGAGES, DEEDS OF TRUST OR OTHER OBLIGATIONS UNDER SEC. 57C OF THE NATIONAL INTERNAL REVENUE CODE OF 1997, AS AMENDED",
        rate_text: "20.0",
    },
];

/// Schedule 1 treaty codes in official order (`treatyCodes.xml`, form 1601FQ).
pub const FORM_1601FQ_TREATIES: &[(&str, &str)] = &[
    ("AU", "Australia"),
    ("AT", "Austria"),
    ("BH", "Bahrain"),
    ("BD", "Bangladesh"),
    ("BE", "Belgium"),
    ("BR", "Brazil"),
    ("BN", "Brunei"),
    ("CA", "Canada"),
    ("CN", "China"),
    ("CZ", "Czech Republic"),
    ("DK", "Denmark"),
    ("FI", "Finland"),
    ("FR", "France"),
    ("DE", "Germany"),
    ("HU", "Hungary"),
    ("IN", "India"),
    ("ID", "Indonesia"),
    ("IL", "Israel"),
    ("IT", "Italy"),
    ("JP", "Japan"),
    ("KR", "Korea"),
    ("KW", "Kuwait"),
    ("MY", "Malaysia"),
    ("MX", "Mexico"),
    ("NL", "Netherlands"),
    ("NZ", "New Zealand"),
    ("NG", "Nigeria"),
    ("NO", "Norway"),
    ("PK", "Pakistan"),
    ("PL", "Poland"),
    ("QA", "Qatar"),
    ("RO", "Romania"),
    ("RU", "Russia"),
    ("SG", "Singapore"),
    ("ES", "Spain"),
    ("LK", "Sri Lanka"),
    ("SE", "Sweden"),
    ("CH", "Switzerland"),
    ("TH", "Thailand"),
    ("TR", "Turkey"),
    ("UAE", "United Arab Emirates"),
    ("US", "USA"),
    ("GB", "United Kingdom"),
];

/// Item 11, category of withholding agent (`CatAgent_P` / `CatAgent_G`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form1601FqCategory {
    Private,
    Government,
}

impl Form1601FqCategory {
    /// The popup list `changedrpATCList` draws for this category.
    pub fn atc_options(self) -> &'static [Form1601FqAtcOption] {
        match self {
            Self::Private => FORM_1601FQ_PRIVATE_ATCS,
            Self::Government => FORM_1601FQ_GOVERNMENT_ATCS,
        }
    }
}

/// Item 13A (`drpSpecialTax`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form1601FqTaxRelief {
    /// Item 13 "Yes" with the blank option chosen.
    Unspecified,
    /// `enableSelTreaty` preselects this one.
    #[default]
    SpecialRate,
    InternationalTaxTreaty,
    Both,
}

impl Form1601FqTaxRelief {
    fn option_value(self) -> &'static str {
        match self {
            Self::Unspecified => "0",
            Self::SpecialRate => "1",
            Self::InternationalTaxTreaty => "2",
            Self::Both => "3",
        }
    }
}

/// One Part II ATC row: Items 14–19 (and "Other Selected ATC" beyond six).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Form1601FqAtcRow {
    /// Position in the category's popup list (the list has duplicate codes
    /// with different rates, so the code alone does not identify a row).
    pub popup_index: usize,
    pub atc_code: String,
    pub tax_base: f64,
    #[serde(default)]
    pub tax_withheld: f64,
}

/// Value of an unused Schedule 1 dropdown once Item 13 is "Yes".
pub const FORM_1601FQ_NOT_APPLICABLE: &str = "NA";

/// One Schedule 1 row (`drpTreatyCode<i>` … `txtReqWithheld<i>`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Form1601FqScheduleRow {
    /// `"NA"` or a code from [`FORM_1601FQ_TREATIES`].
    pub treaty_code: String,
    /// `"NA"` or an ATC code from the 1601-FQ list.
    pub atc_code: String,
    /// Column E.
    pub income_payment: f64,
    /// Column F, percent; `getATCdrpTaxRate` fills it, the filer may change it.
    pub tax_rate: f64,
    /// Column G (computed).
    #[serde(default)]
    pub tax_withheld: f64,
}

impl Default for Form1601FqScheduleRow {
    fn default() -> Self {
        Self {
            treaty_code: FORM_1601FQ_NOT_APPLICABLE.to_string(),
            atc_code: FORM_1601FQ_NOT_APPLICABLE.to_string(),
            income_payment: 0.0,
            tax_rate: 0.0,
            tax_withheld: 0.0,
        }
    }
}

impl Form1601FqScheduleRow {
    pub fn is_unused(&self) -> bool {
        self.treaty_code == FORM_1601FQ_NOT_APPLICABLE
            && self.atc_code == FORM_1601FQ_NOT_APPLICABLE
            && self.income_payment == 0.0
            && self.tax_rate == 0.0
    }
}

/// Whether `code` is an option of the Schedule 1 ATC dropdown.
pub fn form_1601fq_schedule_atc_is_option(code: &str) -> bool {
    code == FORM_1601FQ_NOT_APPLICABLE
        || FORM_1601FQ_PRIVATE_ATCS
            .iter()
            .chain(FORM_1601FQ_GOVERNMENT_ATCS)
            .any(|option| option.code == code)
}

/// Whether `code` is an option of the Schedule 1 treaty dropdown.
pub fn form_1601fq_treaty_is_option(code: &str) -> bool {
    code == FORM_1601FQ_NOT_APPLICABLE || FORM_1601FQ_TREATIES.iter().any(|(c, _)| *c == code)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Form1601FqDraft {
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
    pub category: Option<Form1601FqCategory>,
    pub email: String,
    /// Items 13/13A: `None` is "No".
    #[serde(default)]
    pub tax_relief: Option<Form1601FqTaxRelief>,

    // Part II
    /// ATC rows in popup order (the page always lists them that way).
    #[serde(default)]
    pub schedule: Vec<Form1601FqAtcRow>,
    #[serde(default)]
    pub total_other_tax_withheld: f64,
    /// Item 20.
    #[serde(default)]
    pub taxes_withheld_regular: f64,
    /// Item 21 (Schedule 1 total).
    #[serde(default)]
    pub taxes_withheld_treaty: f64,
    /// Item 22.
    #[serde(default)]
    pub total_taxes_withheld: f64,
    /// Items 23–25.
    #[serde(default)]
    pub remittance_first_month: f64,
    #[serde(default)]
    pub remittance_second_month: f64,
    #[serde(default)]
    pub tax_remitted_previous: f64,
    /// Item 26.
    #[serde(default)]
    pub total_remittances: f64,
    /// Item 27.
    #[serde(default)]
    pub tax_still_due: f64,
    /// Items 28–31.
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
    /// Item 32.
    #[serde(default)]
    pub total_amount_due: f64,

    // Part IV — Schedule 1
    #[serde(default)]
    pub schedule1: Vec<Form1601FqScheduleRow>,
    #[serde(default)]
    pub schedule1_total: f64,

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

impl Form1601FqDraft {
    pub const FORM_CODE: &'static str = "1601FQ";

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
            tax_relief: None,
            schedule: Vec::new(),
            total_other_tax_withheld: 0.0,
            taxes_withheld_regular: 0.0,
            taxes_withheld_treaty: 0.0,
            total_taxes_withheld: 0.0,
            remittance_first_month: 0.0,
            remittance_second_month: 0.0,
            tax_remitted_previous: 0.0,
            total_remittances: 0.0,
            tax_still_due: 0.0,
            surcharge: 0.0,
            interest: 0.0,
            compromise: 0.0,
            total_penalties: 0.0,
            total_amount_due: 0.0,
            schedule1: Vec::new(),
            schedule1_total: 0.0,
            lifecycle: SubmissionLifecycle::default(),
        };
        draft.recompute();
        draft
    }

    /// The popup list for the chosen category (private before Item 11 is
    /// answered, like `init()`).
    pub fn atc_options(&self) -> &'static [Form1601FqAtcOption] {
        self.category
            .unwrap_or(Form1601FqCategory::Private)
            .atc_options()
    }

    /// Item 4. "No" clears Part II and Schedule 1 like `cancelAllCompute`.
    pub fn set_any_tax_withheld(&mut self, withheld: bool) {
        self.any_tax_withheld = Some(withheld);
        if !withheld {
            self.schedule.clear();
            if self.tax_relief.is_some() {
                self.schedule1 = vec![Form1601FqScheduleRow::default(); FORM_1601FQ_SCHEDULE1_ROWS];
            }
        }
        self.recompute();
    }

    /// Item 11. A new category draws a new popup list, so ticked ATCs go.
    pub fn set_category(&mut self, category: Form1601FqCategory) {
        if self.category != Some(category) {
            self.schedule.clear();
        }
        self.category = Some(category);
        self.recompute();
    }

    /// Items 13/13A. "Yes" opens Schedule 1 with every row at "NA"
    /// (`enableSelTreaty`); "No" clears it (`confirmDisableTreaty`).
    pub fn set_tax_relief(&mut self, relief: Option<Form1601FqTaxRelief>) {
        match relief {
            Some(_) if self.tax_relief.is_none() => {
                self.schedule1 = vec![Form1601FqScheduleRow::default(); FORM_1601FQ_SCHEDULE1_ROWS];
            }
            None => self.schedule1.clear(),
            _ => {}
        }
        self.tax_relief = relief;
        self.recompute();
    }

    /// Tick an ATC in the popup by its position in the category's list.
    pub fn add_atc(&mut self, popup_index: usize) -> Result<(), String> {
        if self.any_tax_withheld != Some(true) {
            return Err(
                "Selecting an ATC is not necessary when item no. 4 is set to ' NO '".to_string(),
            );
        }
        if self.category.is_none() {
            return Err("Please select an option for Item 11.".to_string());
        }
        let Some(option) = self.atc_options().get(popup_index) else {
            return Err("Not an ATC on the official 1601-FQ list.".to_string());
        };
        if self
            .schedule
            .iter()
            .any(|row| row.popup_index == popup_index)
        {
            return Err(format!("{} is already selected.", option.code));
        }
        self.schedule.push(Form1601FqAtcRow {
            popup_index,
            atc_code: option.code.to_string(),
            tax_base: 0.0,
            tax_withheld: 0.0,
        });
        self.recompute();
        Ok(())
    }

    /// Untick an ATC.
    pub fn remove_atc(&mut self, popup_index: usize) {
        self.schedule.retain(|row| row.popup_index != popup_index);
        self.recompute();
    }

    /// Choose a Schedule 1 ATC. Like `getATCdrpTaxRate`, the rate becomes the
    /// first entry with that code in the current popup list (0 if none).
    pub fn set_schedule1_atc(&mut self, row: usize, code: &str) {
        let rate = self.schedule1_list_entry(code).map_or(0.0, |o| o.rate());
        if let Some(entry) = self.schedule1.get_mut(row) {
            entry.atc_code = code.to_string();
            entry.tax_rate = rate;
        }
        self.recompute();
    }

    /// `getATCdrpNaturePayment` / `getATCdrpTaxRate`: the first entry with the
    /// code in the current popup list.
    fn schedule1_list_entry(&self, code: &str) -> Option<&'static Form1601FqAtcOption> {
        self.atc_options().iter().find(|option| option.code == code)
    }

    /// Column D of a Schedule 1 row.
    pub fn schedule1_nature(&self, row: &Form1601FqScheduleRow) -> String {
        self.schedule1_list_entry(&row.atc_code)
            .map(|option| option.description.to_string())
            .unwrap_or_else(|| "-".to_string())
    }

    fn row_option(&self, row: &Form1601FqAtcRow) -> Option<&'static Form1601FqAtcOption> {
        self.atc_options()
            .get(row.popup_index)
            .filter(|option| option.code == row.atc_code)
    }

    /// The official compute chain over values held at cents.
    pub fn recompute(&mut self) {
        self.schedule.sort_by_key(|row| row.popup_index);
        if !self.is_amended {
            self.tax_remitted_previous = 0.0;
        }
        let options = self.atc_options();
        let mut total = 0.0;
        let mut other = 0.0;
        for (index, row) in self.schedule.iter_mut().enumerate() {
            row.tax_base = cents(row.tax_base);
            let rate = options
                .get(row.popup_index)
                .filter(|option| option.code == row.atc_code)
                .map_or(0.0, |option| option.rate());
            // getRequiredWithheld: base * rate / 100.
            row.tax_withheld = cents(row.tax_base * rate / 100.0);
            total += row.tax_withheld;
            if index >= FORM_1601FQ_MAIN_ROWS {
                other += row.tax_withheld;
            }
        }
        self.taxes_withheld_regular = cents(total);
        self.total_other_tax_withheld = cents(other);

        let mut schedule1_total = 0.0;
        for row in &mut self.schedule1 {
            row.income_payment = cents(row.income_payment);
            row.tax_rate = cents(row.tax_rate);
            // getReqWithheldCompute: amount * rate / 100.
            row.tax_withheld = cents(row.income_payment * row.tax_rate / 100.0);
            schedule1_total += row.tax_withheld;
        }
        self.schedule1_total = cents(schedule1_total);
        // computeSched1.
        self.taxes_withheld_treaty = self.schedule1_total;

        self.total_taxes_withheld = cents(self.taxes_withheld_regular + self.taxes_withheld_treaty);
        self.remittance_first_month = cents(self.remittance_first_month);
        self.remittance_second_month = cents(self.remittance_second_month);
        self.tax_remitted_previous = cents(self.tax_remitted_previous);
        self.total_remittances = cents(
            self.remittance_first_month + self.remittance_second_month + self.tax_remitted_previous,
        );
        self.tax_still_due = cents(self.total_taxes_withheld - self.total_remittances);
        // The penalty items file their override, else the computed value.
        Self::penalty_declaration().apply(self);
        self.surcharge = cents(self.surcharge);
        self.interest = cents(self.interest);
        self.compromise = cents(self.compromise);
        self.total_penalties = cents(self.surcharge + self.interest + self.compromise);
        self.total_amount_due = cents(self.tax_still_due + self.total_penalties);
    }

    /// `AtcCd<i>` checkboxes in the popup: none before Item 11 is answered,
    /// the category's list, plus the "N/A" row once Item 13 is "Yes".
    fn popup_list_len(&self) -> usize {
        match self.category {
            None => 0,
            Some(category) => category.atc_options().len() + usize::from(self.tax_relief.is_some()),
        }
    }

    fn row_count(&self) -> usize {
        self.schedule.len().max(FORM_1601FQ_MAIN_ROWS)
    }

    /// The print sheet's field map: the upload map plus `derived:` keys
    /// that only shape the print.
    ///
    /// - `derived:tin_digits`: the later pages' header TIN, one 14-digit
    ///   comb (TIN then branch).
    /// - `derived:address_line1` / `derived:line:address_line2`: the
    ///   registered address over its 40-slot first line, broken at a
    ///   space, and the second line (printed whole when longer).
    /// - `derived:line:tax_relief_specify`: "If yes, specify", the relief
    ///   named by `frm1601FQ:drpSpecialTax` (the upload carries the dropdown code).
    /// - `derived:rate{n}_whole` / `derived:rate{n}_tenths`: Part II row
    ///   `n`'s rate over its 2-slot comb and 1-slot tenths box.
    /// - `derived:sched1_drpTreatyCode{i}` / `derived:sched1_drpATCCode{i}`:
    ///   Schedule 1 row `i`'s codes, blank for the page placeholders.
    pub fn to_print_field_map(&self) -> BTreeMap<String, String> {
        let mut fields = self.to_bir_field_map();
        let tin_digits = super::print_map::digits_of(
            &fields,
            &[
                "frm1601FQ:txtPg2TIN1",
                "frm1601FQ:txtPg2TIN2",
                "frm1601FQ:txtPg2TIN3",
                "frm1601FQ:txtPg2BranchCode",
            ],
        );
        let address = self.registered_address.trim().to_uppercase();
        let (line1, line2) = super::print_map::split_print_line(&address, 40);
        let specify = super::print_map::tax_relief_label(
            fields
                .get("frm1601FQ:drpSpecialTax")
                .map_or("", String::as_str),
        )
        .to_string();
        // Part II rows 1-6: the rate (` 20.0`) over its 2-slot whole comb
        // and 1-slot tenths box; Schedule 1's dropdowns print blank for the
        // page's `-` / `NA` placeholders.
        for n in 1..=6 {
            let rate = fields
                .get(&format!("frm1601FQ:txtTaxRate{n}"))
                .map(|rate| rate.trim().to_string())
                .unwrap_or_default();
            let (whole, tenths) = rate.split_once('.').unwrap_or((rate.as_str(), ""));
            fields.insert(format!("derived:rate{n}_whole"), whole.to_string());
            fields.insert(
                format!("derived:rate{n}_tenths"),
                tenths.chars().take(1).collect(),
            );
        }
        for index in 0..5 {
            for key in ["drpTreatyCode", "drpATCCode"] {
                let value = fields
                    .get(&format!("{key}{index}"))
                    .map(|value| value.trim())
                    .filter(|value| !matches!(*value, "-" | FORM_1601FQ_NOT_APPLICABLE))
                    .unwrap_or("")
                    .to_string();
                fields.insert(format!("derived:sched1_{key}{index}"), value);
            }
        }
        for (key, value) in [
            ("tin_digits", tin_digits),
            ("address_line1", line1),
            ("line:address_line2", line2),
            ("line:tax_relief_specify", specify),
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
        let p = |key: &str| format!("frm1601FQ:{key}");
        let flag = |on: bool| on.to_string();
        // computeofTotalWithheldTax calls capital(), which uppercases every
        // text input except txtEmail.
        let text = |value: &str| value.trim().to_uppercase();

        put(&p("txtYear"), self.taxable_year.to_string());
        for quarter in 1..=4u8 {
            put(
                &p(&format!("OptQuarter{quarter}")),
                flag(self.quarter == quarter),
            );
        }
        put(&p("AmendedRtn_1"), flag(self.is_amended));
        put(&p("AmendedRtn_2"), flag(!self.is_amended));
        put(
            &p("TaxWithheld_1"),
            flag(self.any_tax_withheld == Some(true)),
        );
        put(
            &p("TaxWithheld_2"),
            flag(self.any_tax_withheld == Some(false)),
        );
        put(&p("txtSheets"), self.number_of_attached_sheets.to_string());

        let (tin1, tin2, tin3, branch) = split_tin(&self.tin);
        put(&p("txtTIN1"), tin1.clone());
        put(&p("txtTIN2"), tin2.clone());
        put(&p("txtTIN3"), tin3.clone());
        put(&p("txtBranchCode"), branch.clone());
        put(&p("txtRDOCode"), self.rdo_code.trim().to_string());
        put(&p("txtTaxpayerName"), text(&self.taxpayer_name));
        // Address lines 1 and 2 are written back to back into one value.
        put(&p("txtAddress"), text(&self.registered_address));
        put(&p("txtAddress2"), String::new());
        put(&p("txtZipCode"), self.zip_code.trim().to_string());
        put(&p("txtTelNum"), self.contact_number.trim().to_string());
        put(
            &p("CatAgent_P"),
            flag(self.category == Some(Form1601FqCategory::Private)),
        );
        put(
            &p("CatAgent_G"),
            flag(self.category == Some(Form1601FqCategory::Government)),
        );
        put("txtEmail", self.email.trim().to_string());
        put(&p("SpecialTax_1"), flag(self.tax_relief.is_some()));
        put(&p("SpecialTax_2"), flag(self.tax_relief.is_none()));
        put(
            &p("drpSpecialTax"),
            self.tax_relief
                .map_or("0", Form1601FqTaxRelief::option_value)
                .to_string(),
        );

        let options = self.atc_options();
        for index in 0..self.row_count() {
            let n = index + 1;
            match self.schedule.get(index) {
                Some(row) => {
                    let rate = options
                        .get(row.popup_index)
                        .map(|option| format!(" {}", option.rate_text))
                        .unwrap_or_default();
                    put(&p(&format!("txtAtcCode{n}")), row.atc_code.clone());
                    put(&p(&format!("txtTaxBase{n}")), official_amount(row.tax_base));
                    put(&p(&format!("txtTaxRate{n}")), rate);
                    put(
                        &p(&format!("txtTaxbeWithHeld{n}")),
                        official_amount(row.tax_withheld),
                    );
                }
                None => {
                    put(&p(&format!("txtAtcCode{n}")), String::new());
                    put(&p(&format!("txtTaxBase{n}")), String::new());
                    put(&p(&format!("txtTaxRate{n}")), String::new());
                    put(&p(&format!("txtTaxbeWithHeld{n}")), "0.00".to_string());
                }
            }
        }
        put(
            &p("txtTotalOtherTax"),
            if self.schedule.is_empty() {
                String::new()
            } else {
                official_amount(self.total_other_tax_withheld)
            },
        );

        put(&p("txtTax20"), official_amount(self.taxes_withheld_regular));
        put(&p("txtTax21"), official_amount(self.taxes_withheld_treaty));
        put(&p("txtTax22"), official_amount(self.total_taxes_withheld));
        put(&p("txtTax23"), official_amount(self.remittance_first_month));
        put(
            &p("txtTax24"),
            official_amount(self.remittance_second_month),
        );
        put(&p("txtTax25"), official_amount(self.tax_remitted_previous));
        put(&p("txtTax26"), official_amount(self.total_remittances));
        put(&p("txtTax27"), official_amount(self.tax_still_due));
        put(&p("txtTax28"), official_amount(self.surcharge));
        put(&p("txtTax29"), official_amount(self.interest));
        put(&p("txtTax30"), official_amount(self.compromise));
        put(&p("txtTax31"), official_amount(self.total_penalties));
        put(&p("txtTax32"), official_amount(self.total_amount_due));

        // Page 2 header, filled from the registration profile.
        put(&p("txtPg2TIN1"), tin1);
        put(&p("txtPg2TIN2"), tin2);
        put(&p("txtPg2TIN3"), tin3);
        put(&p("txtPg2BranchCode"), branch);
        put(&p("txtPg2TaxpayerName"), text(&self.taxpayer_name));

        for index in 0..FORM_1601FQ_SCHEDULE1_ROWS {
            match (self.tax_relief, self.schedule1.get(index)) {
                (Some(_), Some(row)) => {
                    put(&format!("drpTreatyCode{index}"), row.treaty_code.clone());
                    put(&format!("drpATCCode{index}"), row.atc_code.clone());
                    put(
                        &format!("txtNatIncPayment{index}"),
                        self.schedule1_nature(row),
                    );
                    put(
                        &format!("txtAmtIncomePay{index}"),
                        official_amount(row.income_payment),
                    );
                    put(&format!("txtRate{index}"), official_amount(row.tax_rate));
                    put(
                        &format!("txtReqWithheld{index}"),
                        official_amount(row.tax_withheld),
                    );
                }
                (relief, _) => {
                    let blank = if relief.is_some() {
                        FORM_1601FQ_NOT_APPLICABLE
                    } else {
                        "-"
                    };
                    put(&format!("drpTreatyCode{index}"), blank.to_string());
                    put(&format!("drpATCCode{index}"), blank.to_string());
                    put(&format!("txtNatIncPayment{index}"), "-".to_string());
                    put(&format!("txtAmtIncomePay{index}"), "0.00".to_string());
                    put(&format!("txtRate{index}"), "0.00".to_string());
                    put(&format!("txtReqWithheld{index}"), "0.00".to_string());
                }
            }
        }
        put("txtDvTotalSchedI", official_amount(self.schedule1_total));

        let ticked: Vec<usize> = self.schedule.iter().map(|row| row.popup_index).collect();
        for index in 0..self.popup_list_len() {
            put(
                &format!("AtcCd{}", index + 1),
                flag(ticked.contains(&index)),
            );
        }
        put(&p("txtLineBus"), text(&self.line_of_business));
        // init() shows page 1; the page is validated and submitted from it.
        put(&p("txtCurrentPage"), "1".to_string());
        fields
    }

    /// The generated official layout plus the controls the page draws at run
    /// time for this return (see the module docs).
    pub fn official_layout(&self) -> Result<OfficialLayout, crate::official_xml::OfficialXmlError> {
        let mut layout = crate::official_xml::layout(FORM_1601FQ_FORM_ID)?.clone();
        let position = |layout: &OfficialLayout, key: &str| {
            layout.entries.iter().position(|entry| match entry {
                Entry::Bool { key: k, .. } | Entry::Value { key: k, .. } => k == key,
            })
        };
        let missing =
            |key: &str| crate::official_xml::OfficialXmlError::UnknownKey(key.to_string());
        let anchor = "frm1601FQ:drpSpecialTax";
        let relief = position(&layout, anchor).ok_or_else(|| missing(anchor))?;
        let after = match &layout.entries[relief] {
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
                    format!("frm1601FQ:txtAtcCode{n}"),
                    format!("frm1601FQ:txtTaxBase{n}"),
                    format!("frm1601FQ:txtTaxRate{n}"),
                    format!("frm1601FQ:txtTaxbeWithHeld{n}"),
                ]
            })
            .map(value)
            .collect::<Vec<_>>()
        };

        let final_flag =
            position(&layout, "txtFinalFlag").ok_or_else(|| missing("txtFinalFlag"))?;
        let mut tail: Vec<Entry> = (1..=self.popup_list_len())
            .map(|n| Entry::Bool {
                key: format!("AtcCd{n}"),
                default: "false".to_string(),
                after: after.clone(),
            })
            .collect();
        tail.extend(row_entries(FORM_1601FQ_MAIN_ROWS + 1..self.row_count() + 1));
        layout.entries.splice(final_flag..final_flag, tail);
        Ok(layout)
    }

    /// The exact official submit plaintext.
    pub fn to_bir_xml_payload(&self) -> Result<String, Vec<(String, String)>> {
        self.official_payload()
    }

    /// `validate()` against a given "today" (Items 1 and 2 depend on it).
    pub fn validate_on(&self, today: chrono::NaiveDate) -> Vec<(String, String)> {
        use chrono::Datelike;
        let mut errors = Vec::new();
        let mut err = |field: &str, message: &str| {
            errors.push((field.to_string(), message.to_string()));
        };
        let current_year = u16::try_from(today.year()).unwrap_or(u16::MAX);
        // JavaScript getMonth(): 0 = January.
        let month0 = today.month0();

        let year = self.taxable_year;
        let year_ok = if year == 0 {
            err("taxable_year", "Please enter a valid year on Item 1.");
            false
        } else if year > current_year {
            err(
                "taxable_year",
                "Invalid entry on Item 1. Entry should not be a future date.",
            );
            false
        } else if year < 2018 {
            err(
                "taxable_year",
                "Invalid date entry on Item no.1. Entry should not be lower than 2018.",
            );
            false
        } else {
            true
        };

        // Item 2. The page lets the first quarter through from March.
        let past_year = year < current_year;
        match self.quarter {
            1 if year_ok && !past_year && month0 < 2 => err(
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
        // Address lines of 100 and 50 characters.
        if address.is_empty() || address.chars().count() > 150 {
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
        if email.is_empty()
            || email.len() > 60
            || !email.contains('@')
            || email.contains(char::is_whitespace)
        {
            err("email", "Please enter Email Address on Item 12.");
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

        // Part II ATCs.
        if self.any_tax_withheld == Some(false) && !self.schedule.is_empty() {
            err(
                "schedule",
                "Selecting an ATC is not necessary when item no. 4 is set to ' NO '",
            );
        }
        for (index, row) in self.schedule.iter().enumerate() {
            if self.row_option(row).is_none() {
                err(
                    &format!("schedule[{index}].atc_code"),
                    &format!(
                        "{} is not on the official 1601-FQ list for Item 11.",
                        row.atc_code
                    ),
                );
                continue;
            }
            if self.schedule[..index]
                .iter()
                .any(|other| other.popup_index == row.popup_index)
            {
                err(
                    &format!("schedule[{index}].atc_code"),
                    &format!("{} is already selected.", row.atc_code),
                );
            }
            if row.tax_base == 0.0 {
                err(
                    &format!("schedule[{index}].tax_base"),
                    &format!(
                        "Please enter a valid value for tax base for {}.",
                        row.atc_code
                    ),
                );
            } else if row.tax_base < 0.0
                || row.tax_base >= MAX_AMOUNT
                || !has_cent_precision(row.tax_base)
            {
                err(
                    &format!("schedule[{index}].tax_base"),
                    &format!("Please enter Tax Base for ATC {}.", row.atc_code),
                );
            }
        }

        // Items 13/13A and Schedule 1.
        match self.tax_relief {
            Some(Form1601FqTaxRelief::Unspecified) => err(
                "tax_relief",
                "Invalid entry on Item no.13A. Please specify a treaty.",
            ),
            None if self.schedule1.iter().any(|row| !row.is_unused()) => err(
                "schedule1",
                "You are not availing of tax relief under Special Law or International Tax Treaty. There is no need to\nfill up schedule 1.",
            ),
            _ => {}
        }
        if self.tax_relief.is_some() {
            if self.schedule1.len() > FORM_1601FQ_SCHEDULE1_ROWS {
                err("schedule1", "Schedule 1 has five rows.");
            }
            let used: Vec<(usize, &Form1601FqScheduleRow)> = self
                .schedule1
                .iter()
                .enumerate()
                .filter(|(_, row)| !row.is_unused())
                .collect();
            if used.is_empty() {
                err("schedule1", "Please fill up Schedule 1.");
            } else if self.any_tax_withheld == Some(false) {
                err(
                    "schedule1",
                    "You have no taxed withheld. There is no need to fill up Schedule 1.",
                );
            }
            // ifRequirementMeetSched1, for the rows in use.
            for (index, row) in used {
                if !form_1601fq_treaty_is_option(&row.treaty_code) {
                    err(
                        &format!("schedule1[{index}].treaty_code"),
                        "Please choose a Treaty Code from the list.",
                    );
                }
                if !form_1601fq_schedule_atc_is_option(&row.atc_code) {
                    err(
                        &format!("schedule1[{index}].atc_code"),
                        "Please choose an ATC Code from the list.",
                    );
                }
                if row.income_payment <= 0.0
                    || row.income_payment >= MAX_AMOUNT
                    || !has_cent_precision(row.income_payment)
                {
                    err(
                        &format!("schedule1[{index}].income_payment"),
                        "Please enter Amount of Income Payment.",
                    );
                }
                if !(0.0..100.0).contains(&row.tax_rate) || !has_cent_precision(row.tax_rate) {
                    err(
                        &format!("schedule1[{index}].tax_rate"),
                        "Enter a Schedule 1 tax rate from 0 to below 100 percent.",
                    );
                }
            }
        }

        for (field, value) in [
            ("remittance_first_month", self.remittance_first_month),
            ("remittance_second_month", self.remittance_second_month),
            ("tax_remitted_previous", self.tax_remitted_previous),
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
                "Item 25 applies only to an amended return.",
            );
        }

        let mut expected = self.clone();
        expected.recompute();
        if expected.total_amount_due != self.total_amount_due
            || expected.taxes_withheld_regular != self.taxes_withheld_regular
            || expected.taxes_withheld_treaty != self.taxes_withheld_treaty
            || expected.total_other_tax_withheld != self.total_other_tax_withheld
            || expected.schedule != self.schedule
            || expected.schedule1 != self.schedule1
        {
            err(
                "total_amount_due",
                "Totals are out of date. Recompute the return.",
            );
        }

        errors
    }
}

impl FormValidator for Form1601FqDraft {
    /// `validate()` in order, with its alert texts, plus the input limits the
    /// official page enforces while typing.
    fn validate(&self) -> Vec<(String, String)> {
        self.validate_on(chrono::Local::now().date_naive())
    }
}

impl Form1601FqDraft {
    /// Items 28-30 on the shared penalty model ([`super::penalty_items`]).
    pub fn penalty_declaration() -> PenaltyItems<Self> {
        super::penalty_items::queueable_penalty_items!(
            Self,
            schedule: Withholding,
            items: [
                surcharge: "28 Surcharge",
                interest: "29 Interest",
                compromise: "30 Compromise",
            ],
            controls: [
                "frm1601FQ:txtTax28",
                "frm1601FQ:txtTax29",
                "frm1601FQ:txtTax30",
            ],
            state: penalty,
            basic_tax_due: "Item 27", |d| d.tax_still_due,
        )
    }
}

impl QueueableForm for Form1601FqDraft {
    fn print_field_map(&self) -> BTreeMap<String, String> {
        self.to_print_field_map()
    }
    fn penalty_items() -> Vec<PenaltyItems<Self>> {
        vec![Self::penalty_declaration()]
    }
    const FORM_CODE: &'static str = "1601FQ";
    /// Official `formType` (`1601FQ`).
    const FORM_TYPE: &'static str = "1601FQ";
    const LAYOUT_ID: &'static str = FORM_1601FQ_FORM_ID;

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

    pub(crate) fn sample() -> Form1601FqDraft {
        let mut draft = Form1601FqDraft {
            penalty: crate::forms::penalty_items::PenaltyState::new(None),
            id: None,
            tin: "12345678800000".to_string(),
            taxable_year: 2025,
            quarter: 2,
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
            tax_relief: None,
            schedule: Vec::new(),
            total_other_tax_withheld: 0.0,
            taxes_withheld_regular: 0.0,
            taxes_withheld_treaty: 0.0,
            total_taxes_withheld: 0.0,
            remittance_first_month: 0.0,
            remittance_second_month: 0.0,
            tax_remitted_previous: 0.0,
            total_remittances: 0.0,
            tax_still_due: 0.0,
            surcharge: 0.0,
            interest: 0.0,
            compromise: 0.0,
            total_penalties: 0.0,
            total_amount_due: 0.0,
            schedule1: Vec::new(),
            schedule1_total: 0.0,
            lifecycle: SubmissionLifecycle::default(),
        };
        // This sample enters its penalties by hand.
        draft.keep_entered_penalties();
        draft.set_any_tax_withheld(true);
        draft.set_category(Form1601FqCategory::Private);
        draft.set_tax_relief(Some(Form1601FqTaxRelief::InternationalTaxTreaty));
        // Ticked out of order; WC212 appears twice in the list (30% and 25%).
        for index in [29, 0, 15, 2, 28] {
            draft.add_atc(index).unwrap();
        }
        for (row, base) in
            draft
                .schedule
                .iter_mut()
                .zip([123_456.789, 1_000.005, 50_000.0, 77_777.775, 999.99])
        {
            row.tax_base = base;
        }
        draft.schedule1[0].treaty_code = "US".into();
        draft.set_schedule1_atc(0, "WI330");
        draft.schedule1[0].income_payment = 100_000.005;
        draft.schedule1[0].tax_rate = 15.0;
        draft.schedule1[1].treaty_code = "JP".into();
        draft.set_schedule1_atc(1, "WC180");
        draft.schedule1[1].income_payment = 50_000.0;
        draft.remittance_first_month = 500.0;
        draft.remittance_second_month = 1_000.505;
        draft.surcharge = 25.5;
        draft.interest = 10.0;
        draft.keep_entered_penalties();
        draft.recompute();
        draft
    }

    fn messages(draft: &Form1601FqDraft) -> Vec<String> {
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
        let withheld: Vec<f64> = draft.schedule.iter().map(|r| r.tax_withheld).collect();
        assert_eq!(withheld, [24_691.36, 200.0, 15_000.0, 7_777.78, 250.0]);
        assert_eq!(draft.schedule[3].tax_base, 77_777.77);
        assert_eq!(draft.taxes_withheld_regular, 47_919.14);
        assert_eq!(draft.schedule1[0].tax_withheld, 15_000.0);
        assert_eq!(draft.schedule1[1].tax_rate, 20.0);
        assert_eq!(draft.taxes_withheld_treaty, 25_000.0);
        assert_eq!(draft.total_taxes_withheld, 72_919.14);
        assert_eq!(draft.total_remittances, 1_500.51);
        assert_eq!(draft.tax_still_due, 71_418.63);
        assert_eq!(draft.total_penalties, 35.5);
        assert_eq!(draft.total_amount_due, 71_454.13);
        assert!(messages(&draft).is_empty(), "{:?}", messages(&draft));
    }

    #[test]
    fn field_map_uses_official_formats() {
        let draft = sample();
        let fields = draft.to_bir_field_map();
        assert_eq!(fields["frm1601FQ:txtAtcCode3"], "WC212");
        assert_eq!(fields["frm1601FQ:txtTaxRate3"], " 30.0");
        assert_eq!(fields["frm1601FQ:txtTaxRate5"], " 25.0");
        assert_eq!(fields["frm1601FQ:txtTaxbeWithHeld6"], "0.00");
        assert_eq!(fields["frm1601FQ:drpSpecialTax"], "2");
        assert_eq!(fields["drpTreatyCode1"], "JP");
        assert_eq!(fields["txtRate0"], "15.00");
        assert_eq!(
            fields["txtNatIncPayment1"],
            "INTEREST ON FOREIGN LOANS PAYABLE TO NON-RESIDENT FOREIGN CORPORATIONS (NRFCS)"
        );
        assert_eq!(fields["drpATCCode2"], "NA");
        assert_eq!(fields["txtNatIncPayment2"], "-");
        assert_eq!(
            fields["frm1601FQ:txtPg2TaxpayerName"],
            "SAMPLE TAXPAYER INC"
        );
        assert_eq!(fields["AtcCd41"], "false");
        assert_eq!(fields["AtcCd30"], "true");
        assert_eq!(
            draft.submission_filename(),
            "12345678800000-1601FQ-2025Q2#sample.taxpayer@example.com#.xml"
        );
        let layout = draft.official_layout().unwrap();
        let keys = layout.keys();
        for key in fields.keys() {
            assert!(keys.contains(key.as_str()), "{key}");
        }
        assert!(draft.to_bir_xml_payload().is_ok());
    }

    #[test]
    fn no_relief_keeps_schedule_one_blank() {
        let mut draft = sample();
        draft.set_tax_relief(None);
        assert!(draft.schedule1.is_empty());
        assert_eq!(draft.taxes_withheld_treaty, 0.0);
        let fields = draft.to_bir_field_map();
        assert_eq!(fields["drpTreatyCode0"], "-");
        assert_eq!(fields["frm1601FQ:drpSpecialTax"], "0");
        // No "N/A" special-law row in the popup.
        assert!(!fields.contains_key("AtcCd41"));
        assert!(messages(&draft).is_empty(), "{:?}", messages(&draft));
    }

    #[test]
    fn period_codes_round_trip() {
        let draft = sample();
        assert_eq!(draft.period_code(), "2025Q2");
        assert_eq!(
            Form1601FqDraft::parse_period_code("2025Q2"),
            Some((2025, FilingPeriod::Quarterly(2)))
        );
        assert_eq!(Form1601FqDraft::parse_period_code("2025Q0"), None);
        assert_eq!(Form1601FqDraft::parse_period_code("062025"), None);
    }

    #[test]
    fn official_negative_cases_are_rejected_with_their_alerts() {
        let check = |mutate: &dyn Fn(&mut Form1601FqDraft), expected: &str| {
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
            "Invalid entry on Item 1. Entry should not be a future date.",
        );
        check(
            &|d| d.taxable_year = 2017,
            "Invalid date entry on Item no.1. Entry should not be lower than 2018.",
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
            &|d| d.tin = "1234".into(),
            "Please enter a valid TIN number on Item 6.",
        );
        check(
            &|d| d.rdo_code = "999".into(),
            "Please enter a valid RDO Code on Item 7.",
        );
        check(
            &|d| d.taxpayer_name.clear(),
            "Please enter a valid Taxpayer Name on Item 8.",
        );
        check(
            &|d| d.contact_number.clear(),
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
            "Please enter Email Address on Item 12.",
        );
        check(
            &|d| d.category = None,
            "Please select an option for Item 11.",
        );
        check(
            &|d| {
                d.schedule[1].tax_base = 0.0;
                d.recompute();
            },
            "Please enter a valid value for tax base for WI250.",
        );
        check(
            &|d| {
                d.schedule[1].tax_base = -5.0;
                d.recompute();
            },
            "Please enter Tax Base for ATC WI250.",
        );
        check(
            &|d| d.tax_relief = Some(Form1601FqTaxRelief::Unspecified),
            "Invalid entry on Item no.13A. Please specify a treaty.",
        );
        check(
            &|d| {
                d.schedule1 = vec![Form1601FqScheduleRow::default(); 5];
                d.recompute();
            },
            "Please fill up Schedule 1.",
        );
        check(
            &|d| {
                d.schedule1[2].income_payment = 10.0;
                d.schedule1[2].treaty_code = "-".into();
                d.recompute();
            },
            "Please choose a Treaty Code from the list.",
        );
        check(
            &|d| {
                d.schedule1[2].income_payment = 10.0;
                d.schedule1[2].atc_code = "WI999".into();
                d.recompute();
            },
            "Please choose an ATC Code from the list.",
        );
        check(
            &|d| {
                d.schedule1[2].treaty_code = "US".into();
                d.recompute();
            },
            "Please enter Amount of Income Payment.",
        );
        check(
            &|d| {
                d.tax_relief = None;
                d.recompute();
            },
            "You are not availing of tax relief under Special Law or International Tax Treaty. There is no need to\nfill up schedule 1.",
        );
        check(
            &|d| d.any_tax_withheld = Some(false),
            "Selecting an ATC is not necessary when item no. 4 is set to ' NO '",
        );
    }

    #[test]
    fn first_quarter_opens_in_march() {
        let mut draft = sample();
        draft.taxable_year = 2026;
        draft.quarter = 1;
        let february = NaiveDate::from_ymd_opt(2026, 2, 28).unwrap();
        let march = NaiveDate::from_ymd_opt(2026, 3, 1).unwrap();
        assert!(
            draft
                .validate_on(february)
                .iter()
                .any(|(_, m)| m.starts_with("Unable to select first Quarter"))
        );
        assert!(
            !draft
                .validate_on(march)
                .iter()
                .any(|(_, m)| m.starts_with("Unable"))
        );
    }

    #[test]
    fn government_list_and_other_selected_atcs() {
        let mut draft = sample();
        draft.set_category(Form1601FqCategory::Government);
        assert!(draft.schedule.is_empty());
        for index in 0..8 {
            draft.add_atc(index).unwrap();
            draft.schedule[index].tax_base = 100.0 * (index + 1) as f64;
        }
        draft.recompute();
        assert_eq!(draft.popup_list_len(), 40);
        // A category change keeps the rate typed into Schedule 1.
        assert_eq!(draft.schedule1[0].tax_rate, 15.0);
        let fields = draft.to_bir_field_map();
        assert_eq!(fields["frm1601FQ:txtAtcCode1"], "WI203");
        assert!(fields.contains_key("frm1601FQ:txtAtcCode8"));
        let payload = draft.official_payload().unwrap();
        let other = payload.find("<div>frm1601FQ:txtAtcCode7=").unwrap();
        assert!(payload.find("<div>AtcCd40=").unwrap() < other);
        assert!(other < payload.find("<div>txtFinalFlag=").unwrap());
    }

    #[test]
    fn queue_and_revalidate_through_the_generic_path() {
        let mut draft = sample();
        draft
            .queue(crate::filing_queue::QueueAuthSource::Gui)
            .unwrap();
        assert!(draft.revalidate_queued_before_submission().is_ok());
        draft.interest = 99.0;
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
            <Form1601FqDraft as FormValidator>::validate(&draft)
                .iter()
                .all(|(field, _)| field != "surcharge"),
            "a large two-decimal amount must be accepted"
        );
        draft.surcharge = 1.005;
        assert!(
            <Form1601FqDraft as FormValidator>::validate(&draft)
                .iter()
                .any(|(field, _)| field == "surcharge"),
            "a three-decimal amount must still be refused"
        );
    }
}
