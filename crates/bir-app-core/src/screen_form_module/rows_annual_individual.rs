//! PR 7: the annual individual income tax returns (1700, 1701, 1701A,
//! 1701MS). What the host writes, closes or reports on these pages beyond
//! the probed bindings:
//!
//! - The spouse part follows the printed civil status (rule 6, and for 1701
//!   an extension of maintainer decision 11, made for 1701Q): the "If
//!   married, spouse has income?" item needs Married, the filing status
//!   item needs Married and Yes, and the spouse's background part needs a
//!   joint return, the forms' own `is_joint` / `spouse_column_active`
//!   (1701MS: item 4 Married and item 5 Jointly). While a part is closed a
//!   write there is refused in the filer's words ([`closed_refusal`]) and
//!   rule 6 fades its boxes with what opens them ([`gated`]); the tags are
//!   app/forms/condition-labels.json's, the conditions the reviewed
//!   [`corpus_gaps`] (the rules corpus records none for these forms).
//! - The spouse's TIN, and on 1701 the Schedule 1 employers' TINs: one draft
//!   field shown over four boxes, written box by box the way 1700's item 18
//!   is (`rows_1700`).
//! - An edit that removes or replaces what the filer typed elsewhere (an
//!   answer that closes the spouse part, Amended Return No, a taxpayer type
//!   or ATC whose schedule the form then clears) is reported ([`removes`]),
//!   so the page asks first and offers Undo.
//! - An official message is drawn under the item it cites
//!   ([`cited_keys`]); its text stays official.
//! - What a few boxes show (display only): 1700's email as typed, and its
//!   spouse RDO and number of attachments blank until there is a value.
//!
//! Field keys, validation, the official XML and the print map are the
//! forms' own and unchanged.

use std::collections::{BTreeMap, BTreeSet};

use bir_core::forms::agent::draft_locks::annual_individual as draft_locks;
use bir_core::forms::agent::{self, AgentFormKind, Binding};
use bir_core::forms::form_1700::{Form1700CivilStatus, Form1700Draft};
use bir_core::forms::form_1701::{Form1701CivilStatus, Form1701Draft};
use bir_core::forms::form_1701a::{Form1701ACivilStatus, Form1701ADraft};
use bir_core::forms::form_1701ms::{Form1701MsCivilStatus, Form1701MsDraft};
use serde_json::{Map, Value, json};

/// The forms of this module.
const FORMS: [&str; 4] = ["1700", "1701", "1701A", "1701MS"];

fn is_mine(kind: AgentFormKind) -> bool {
    FORMS.contains(&kind.form_code())
}

/// What a spouse-part box needs before it is open.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Gate {
    /// The civil status item answered Married.
    Married,
    /// Married, and "spouse has income?" answered Yes.
    SpouseIncome,
    /// A joint return: the spouse's column is on the return.
    Joint,
    /// 1701MS Part II item 20 of a column: open once that column's item 19
    /// (income tax due at the special/preferential rate) is more than zero
    /// (form_1701ms.rs `share_open`, the official toggleShareOfOtherNo20*);
    /// column B also needs a joint return.
    TaxDue { spouse: bool },
    /// 1701MS item 16 (the tax rate) of a column: open while that column's
    /// item 12 is Exempt or Special/Preferential Rate (form_1701ms.rs
    /// `relief_items_open`; `normalize` zeroes `special_rate` otherwise);
    /// column B also needs a joint return.
    Relief { spouse: bool },
}

/// One gated box: `(form, key, gate, condition)`. The condition is the
/// reviewed text the page's rule 6 reads (app/forms/conditions.json via
/// `conditions::tests::derived`); its tag is in condition-labels.json. A
/// condition may need more than the gate (a foreign tax number needs its
/// Yes too): the form's own compute clears the box then, which rule 6 sees
/// the ordinary way.
type GatedBox = (&'static str, &'static str, Gate, &'static str);

const C1700_MARRIED: &str = "Item 15 Civil Status is Married.";
const C1700_INCOME: &str = "Item 15 is Married and Item 16 is Yes.";
const C1700_JOINT: &str = "A joint return: Item 15 Married, Item 16 Yes and Item 17 Joint Filing.";
const C1700_JOINT_FTN: &str = "A joint return (Items 15 to 17) and Item 24 Yes.";

const C1701_MARRIED: &str = "Item 16 Civil Status is Married.";
const C1701_INCOME: &str = "Item 16 is Married and Item 17 is Yes.";
const C1701_JOINT: &str = "A joint return: Item 16 Married, Item 17 Yes and Item 18 Joint Filing.";
const C1701_JOINT_FTN: &str = "A joint return (Items 16 to 18) and Part IV Item 8 Yes.";
const C1701_JOINT_GRADUATED: &str =
    "A joint return (Items 16 to 18) and Part IV Item 12 Graduated Rates.";
const C1701_JOINT_COMPENSATION: &str =
    "A joint return (Items 16 to 18) and Part IV Item 4 II011, II013 or II016.";

const C1701A_MARRIED: &str = "Item 16 Civil Status is Married.";
const C1701A_INCOME: &str = "Item 16 is Married and Item 17 is Yes.";
const C1701A_JOINT: &str = "A joint return: Item 16 Married, Item 17 Yes and Item 18 Joint Filing.";
const C1701A_JOINT_FTN: &str = "A joint return (Items 16 to 18) and Item 73 Yes.";

const C1701MS_MARRIED: &str = "Item 4 Civil Status is Married.";
const C1701MS_JOINT: &str = "A joint return: Item 4 Married and Item 5 Jointly.";
const C1701MS_JOINT_RELIEF: &str =
    "A joint return (Items 4 and 5) and Item 12 column B Exempt or Special/Preferential Rate.";
const C1701MS_JOINT_DEDUCTION: &str = "A joint return (Items 4 and 5) and, in column B, Item 12 Graduated Income Tax Rates with business income at Item 11.";
/// Column B's amounts: the gate is the joint return; once joint, the
/// form's own compute clears what column B's answers close, as for column A
/// (Schedule A for compensation, item 12's rate, an amended return), which
/// rule 6 tries the ordinary way.
const C1701MS_JOINT_COLUMN: &str = "A joint return (Items 4 and 5), and in column B the answers that open the same box of column A.";
const C1701MS_RELIEF_A: &str = "Item 12 column A Exempt or Special/Preferential Rate.";
const C1701MS_SHARE_A: &str =
    "Item 19 column A, Income Tax Due at the Special/Preferential Rate, is more than zero.";
const C1701MS_SHARE_B: &str = "A joint return (Items 4 and 5) and Item 19 column B, Income Tax Due at the Special/Preferential Rate, is more than zero.";

const GATED: &[GatedBox] = &[
    // 1700 items 16-25 (html-frozen/1700-2018/writer-cells.json "16 If
    // married, spouse has income?", "17 Filing Status", 18-25 Part II).
    (
        "1700",
        "frm1700:rdoPg1I16SpouseHasIncomeY",
        Gate::Married,
        C1700_MARRIED,
    ),
    (
        "1700",
        "frm1700:rdoPg1I16SpouseHasIncomeN",
        Gate::Married,
        C1700_MARRIED,
    ),
    (
        "1700",
        "frm1700:rdoPg1I17FilingStatusJ",
        Gate::SpouseIncome,
        C1700_INCOME,
    ),
    (
        "1700",
        "frm1700:rdoPg1I17FilingStatusS",
        Gate::SpouseIncome,
        C1700_INCOME,
    ),
    ("1700", "frm1700:txtPg1I18STIN1", Gate::Joint, C1700_JOINT),
    ("1700", "frm1700:txtPg1I18STIN2", Gate::Joint, C1700_JOINT),
    ("1700", "frm1700:txtPg1I18STIN3", Gate::Joint, C1700_JOINT),
    (
        "1700",
        "frm1700:txtPg1I18SBranchCode",
        Gate::Joint,
        C1700_JOINT,
    ),
    ("1700", "frm1700:txtSpouseRDOCode", Gate::Joint, C1700_JOINT),
    (
        "1700",
        "frm1700:rdoPg1I20SpouseTaxpayerTypeE",
        Gate::Joint,
        C1700_JOINT,
    ),
    (
        "1700",
        "frm1700:rdoPg1I20SpouseTaxpayerTypeN",
        Gate::Joint,
        C1700_JOINT,
    ),
    (
        "1700",
        "frm1700:txtPg1I21SpouseName",
        Gate::Joint,
        C1700_JOINT,
    ),
    (
        "1700",
        "frm1700:txtPg1I22TSpouseTelNum",
        Gate::Joint,
        C1700_JOINT,
    ),
    (
        "1700",
        "frm1700:txtPg1I23SpouseCitizenship",
        Gate::Joint,
        C1700_JOINT,
    ),
    (
        "1700",
        "frm1700:rdoPg1I24SpouseForeignTaxCreditsYes",
        Gate::Joint,
        C1700_JOINT,
    ),
    (
        "1700",
        "frm1700:rdoPg1I24SpouseForeignTaxCreditsNo",
        Gate::Joint,
        C1700_JOINT,
    ),
    (
        "1700",
        "frm1700:txtPg1I25SpouseForeignTaxNumber",
        Gate::Joint,
        C1700_JOINT_FTN,
    ),
    // 1701 items 17-18 and Part IV items 1-12A.
    (
        "1701",
        "frm1701:rdoPg1I17SpouseIncomeYes",
        Gate::Married,
        C1701_MARRIED,
    ),
    (
        "1701",
        "frm1701:rdoPg1I17SpouseIncomeNo",
        Gate::Married,
        C1701_MARRIED,
    ),
    (
        "1701",
        "frm1701:rdoPg1I18FilingStatusJ",
        Gate::SpouseIncome,
        C1701_INCOME,
    ),
    (
        "1701",
        "frm1701:rdoPg1I18FilingStatusS",
        Gate::SpouseIncome,
        C1701_INCOME,
    ),
    ("1701", "frm1701:txtPg2I1TIN1", Gate::Joint, C1701_JOINT),
    ("1701", "frm1701:txtPg2I1TIN2", Gate::Joint, C1701_JOINT),
    ("1701", "frm1701:txtPg2I1TIN3", Gate::Joint, C1701_JOINT),
    (
        "1701",
        "frm1701:txtPg2I1BranchCode",
        Gate::Joint,
        C1701_JOINT,
    ),
    (
        "1701",
        "frm1701:txtPg2I2SpouseRDOCode",
        Gate::Joint,
        C1701_JOINT,
    ),
    (
        "1701",
        "frm1701:rdoPg2I3SpouseTypeS",
        Gate::Joint,
        C1701_JOINT,
    ),
    (
        "1701",
        "frm1701:rdoPg2I3SpouseTypeP",
        Gate::Joint,
        C1701_JOINT,
    ),
    (
        "1701",
        "frm1701:rdoPg2I3SpouseTypeC",
        Gate::Joint,
        C1701_JOINT,
    ),
    (
        "1701",
        "frm1701:rdoPg2I4ATC_II012",
        Gate::Joint,
        C1701_JOINT,
    ),
    (
        "1701",
        "frm1701:rdoPg2I4ATC_II014",
        Gate::Joint,
        C1701_JOINT,
    ),
    (
        "1701",
        "frm1701:rdoPg2I4ATC_II013",
        Gate::Joint,
        C1701_JOINT,
    ),
    (
        "1701",
        "frm1701:rdoPg2I4ATC_II011",
        Gate::Joint,
        C1701_JOINT,
    ),
    (
        "1701",
        "frm1701:rdoPg2I4ATC_II015",
        Gate::Joint,
        C1701_JOINT,
    ),
    (
        "1701",
        "frm1701:rdoPg2I4ATC_II017",
        Gate::Joint,
        C1701_JOINT,
    ),
    (
        "1701",
        "frm1701:rdoPg2I4ATC_II016",
        Gate::Joint,
        C1701_JOINT,
    ),
    (
        "1701",
        "frm1701:txtPg2I5SpouseName",
        Gate::Joint,
        C1701_JOINT,
    ),
    ("1701", "frm1701:txtPg2I6TelNum", Gate::Joint, C1701_JOINT),
    (
        "1701",
        "frm1701:txtPg2I7Citizenship",
        Gate::Joint,
        C1701_JOINT,
    ),
    (
        "1701",
        "frm1701:rdoPg2I8ForeignTaxCreditsYes",
        Gate::Joint,
        C1701_JOINT,
    ),
    (
        "1701",
        "frm1701:rdoPg2I8ForeignTaxCreditsNo",
        Gate::Joint,
        C1701_JOINT,
    ),
    (
        "1701",
        "frm1701:txtPg2I9ForeignTaxNumber",
        Gate::Joint,
        C1701_JOINT_FTN,
    ),
    (
        "1701",
        "frm1701:rdoPg2I10IncomeExemptYes",
        Gate::Joint,
        C1701_JOINT,
    ),
    (
        "1701",
        "frm1701:rdoPg2I10IncomeExemptNo",
        Gate::Joint,
        C1701_JOINT,
    ),
    (
        "1701",
        "frm1701:rdoPg2I11IncomeSpecialYes",
        Gate::Joint,
        C1701_JOINT,
    ),
    (
        "1701",
        "frm1701:rdoPg2I11IncomeSpecialNo",
        Gate::Joint,
        C1701_JOINT,
    ),
    (
        "1701",
        "frm1701:rdoPg2I12TaxRateG",
        Gate::Joint,
        C1701_JOINT,
    ),
    (
        "1701",
        "frm1701:rdoPg2I12TaxRateP",
        Gate::Joint,
        C1701_JOINT,
    ),
    (
        "1701",
        "frm1701:rdoPg2I12AMethodDeductionI",
        Gate::Joint,
        C1701_JOINT_GRADUATED,
    ),
    (
        "1701",
        "frm1701:rdoPg2I12AMethodDeductionO",
        Gate::Joint,
        C1701_JOINT_GRADUATED,
    ),
    // Schedule 1's Spouse marks: the field map files a row for the spouse
    // only while the spouse's ATC has compensation income
    // (form_1701_official.rs `employer_row_allowed`).
    (
        "1701",
        "frm1701:chkPg2IShed1a_1Spouse",
        Gate::Joint,
        C1701_JOINT_COMPENSATION,
    ),
    (
        "1701",
        "frm1701:chkPg2IShed2a_2Spouse",
        Gate::Joint,
        C1701_JOINT_COMPENSATION,
    ),
    // 1701A items 17-18 and Part V items 66-75.
    (
        "1701A",
        "frm1701A:optSpouseIncome_1",
        Gate::Married,
        C1701A_MARRIED,
    ),
    (
        "1701A",
        "frm1701A:optSpouseIncome_2",
        Gate::Married,
        C1701A_MARRIED,
    ),
    (
        "1701A",
        "frm1701A:optFilingStatus_1",
        Gate::SpouseIncome,
        C1701A_INCOME,
    ),
    (
        "1701A",
        "frm1701A:optFilingStatus_2",
        Gate::SpouseIncome,
        C1701A_INCOME,
    ),
    ("1701A", "frm1701A:txtSpouseTIN1", Gate::Joint, C1701A_JOINT),
    ("1701A", "frm1701A:txtSpouseTIN2", Gate::Joint, C1701A_JOINT),
    ("1701A", "frm1701A:txtSpouseTIN3", Gate::Joint, C1701A_JOINT),
    (
        "1701A",
        "frm1701A:txtSpouseBranchCode",
        Gate::Joint,
        C1701A_JOINT,
    ),
    (
        "1701A",
        "frm1701A:txtSpouseRDOCode",
        Gate::Joint,
        C1701A_JOINT,
    ),
    (
        "1701A",
        "frm1701A:optSpouseTaxType_1",
        Gate::Joint,
        C1701A_JOINT,
    ),
    (
        "1701A",
        "frm1701A:optSpouseTaxType_2",
        Gate::Joint,
        C1701A_JOINT,
    ),
    ("1701A", "frm1701A:txtSpouseName", Gate::Joint, C1701A_JOINT),
    (
        "1701A",
        "frm1701A:txtSpouseTelNum",
        Gate::Joint,
        C1701A_JOINT,
    ),
    (
        "1701A",
        "frm1701A:txtSpouseCitizenship",
        Gate::Joint,
        C1701A_JOINT,
    ),
    (
        "1701A",
        "frm1701A:optSpouseFTC_1",
        Gate::Joint,
        C1701A_JOINT,
    ),
    (
        "1701A",
        "frm1701A:optSpouseFTC_2",
        Gate::Joint,
        C1701A_JOINT,
    ),
    (
        "1701A",
        "frm1701A:txtSpouseFTN",
        Gate::Joint,
        C1701A_JOINT_FTN,
    ),
    // 1701MS item 5 and Part I column B.
    (
        "1701MS",
        "frm1701MS:txtIfMarriedJointlyNo5",
        Gate::Married,
        C1701MS_MARRIED,
    ),
    (
        "1701MS",
        "frm1701MS:txtIfMarriedSeparatelyNo5",
        Gate::Married,
        C1701MS_MARRIED,
    ),
    (
        "1701MS",
        "frm1701MS:txtIfMarriedNotApplicableNo5",
        Gate::Married,
        C1701MS_MARRIED,
    ),
    (
        "1701MS",
        "frm1701MS:txtPg2TIN1Spouse",
        Gate::Joint,
        C1701MS_JOINT,
    ),
    (
        "1701MS",
        "frm1701MS:txtPg2TIN2Spouse",
        Gate::Joint,
        C1701MS_JOINT,
    ),
    (
        "1701MS",
        "frm1701MS:txtPg2TIN3Spouse",
        Gate::Joint,
        C1701MS_JOINT,
    ),
    (
        "1701MS",
        "frm1701MS:txtPg2BranchCodeSpouse",
        Gate::Joint,
        C1701MS_JOINT,
    ),
    (
        "1701MS",
        "frm1701MS:txtRDOCodeb",
        Gate::Joint,
        C1701MS_JOINT,
    ),
    (
        "1701MS",
        "frm1701MS:txtTaxpayerNo8b",
        Gate::Joint,
        C1701MS_JOINT,
    ),
    ("1701MS", "frm1701MS:txtEmailb", Gate::Joint, C1701MS_JOINT),
    (
        "1701MS",
        "frm1701MS:txtTelNum10b",
        Gate::Joint,
        C1701MS_JOINT,
    ),
    (
        "1701MS",
        "frm1701MS:txtTaxpayerNo11a1",
        Gate::Joint,
        C1701MS_JOINT,
    ),
    (
        "1701MS",
        "frm1701MS:txtTaxpayerNo11b1",
        Gate::Joint,
        C1701MS_JOINT,
    ),
    (
        "1701MS",
        "frm1701MS:txtTaxpayerNo11c1",
        Gate::Joint,
        C1701MS_JOINT,
    ),
    (
        "1701MS",
        "frm1701MS:txtTaxpayerNo11d1",
        Gate::Joint,
        C1701MS_JOINT,
    ),
    (
        "1701MS",
        "frm1701MS:txtTaxpayerNo12b1",
        Gate::Joint,
        C1701MS_JOINT,
    ),
    (
        "1701MS",
        "frm1701MS:txtTaxpayerNo12c1",
        Gate::Joint,
        C1701MS_JOINT,
    ),
    (
        "1701MS",
        "frm1701MS:txtTaxpayerNo12d1",
        Gate::Joint,
        C1701MS_JOINT,
    ),
    (
        "1701MS",
        "frm1701MS:txtTaxpayerNo12e1",
        Gate::Joint,
        C1701MS_JOINT,
    ),
    (
        "1701MS",
        "frm1701MS:txtTaxpayerNo13b",
        Gate::Joint,
        C1701MS_JOINT_RELIEF,
    ),
    (
        "1701MS",
        "frm1701MS:txtTaxpayerNo14b",
        Gate::Joint,
        C1701MS_JOINT_RELIEF,
    ),
    (
        "1701MS",
        "frm1701MS:txtTaxpayerNo15b",
        Gate::Joint,
        C1701MS_JOINT_RELIEF,
    ),
    (
        "1701MS",
        "frm1701MS:txtTaxpayerNo16b",
        Gate::Joint,
        C1701MS_JOINT_RELIEF,
    ),
    (
        "1701MS",
        "frm1701MS:txtTaxpayerNo16b1",
        Gate::Joint,
        C1701MS_JOINT_RELIEF,
    ),
    (
        "1701MS",
        "frm1701MS:txtSpouseNo17a",
        Gate::Joint,
        C1701MS_JOINT_DEDUCTION,
    ),
    (
        "1701MS",
        "frm1701MS:txtSpouseNo17b",
        Gate::Joint,
        C1701MS_JOINT_DEDUCTION,
    ),
    // These, like every gate here, are host locks (lesson 15), not
    // validation: a closed box refuses the write in the host and rule 6
    // fades it; no message is added and the form's checks are unchanged.
    // 1701MS item 18 column B and column B's amounts (PR 7 review): the
    // form resets the whole spouse column off a joint return
    // (form_1701ms.rs `recompute`: "marriedJointly / toggleLine5 reset
    // every spouse control"), so they take a write only on one.
    (
        "1701MS",
        "taxCodeDropdownSpouse",
        Gate::Joint,
        C1701MS_JOINT,
    ),
    (
        "1701MS",
        "frm1701MS:txtGrossb",
        Gate::Joint,
        C1701MS_JOINT_COLUMN,
    ),
    (
        "1701MS",
        "frm1701MS:txtLessNonTaxableb",
        Gate::Joint,
        C1701MS_JOINT_COLUMN,
    ),
    (
        "1701MS",
        "frm1701MS:txtSalesRevenueNo5b",
        Gate::Joint,
        C1701MS_JOINT_COLUMN,
    ),
    (
        "1701MS",
        "frm1701MS:txtSalesReturnsNo6b",
        Gate::Joint,
        C1701MS_JOINT_COLUMN,
    ),
    (
        "1701MS",
        "frm1701MS:txtCostofSalesNo8b",
        Gate::Joint,
        C1701MS_JOINT_COLUMN,
    ),
    (
        "1701MS",
        "frm1701MS:txtAllowableItemizedDeductionsNo10aSpouse",
        Gate::Joint,
        C1701MS_JOINT_COLUMN,
    ),
    (
        "1701MS",
        "frm1701MS:txtSpecialAllowableNo10b",
        Gate::Joint,
        C1701MS_JOINT_COLUMN,
    ),
    (
        "1701MS",
        "frm1701MS:txtNetOperatingLossNo10b",
        Gate::Joint,
        C1701MS_JOINT_COLUMN,
    ),
    (
        "1701MS",
        "frm1701MS:txtNonOperatingIncomeNo13b",
        Gate::Joint,
        C1701MS_JOINT_COLUMN,
    ),
    (
        "1701MS",
        "frm1701MS:txtSalesRevenuesFeesANo19b",
        Gate::Joint,
        C1701MS_JOINT_COLUMN,
    ),
    (
        "1701MS",
        "frm1701MS:txtOtherNonOperatingIncomeNo20b",
        Gate::Joint,
        C1701MS_JOINT_COLUMN,
    ),
    (
        "1701MS",
        "frm1701MS:txtTaxDueAllowedNo26b",
        Gate::Joint,
        C1701MS_JOINT_COLUMN,
    ),
    (
        "1701MS",
        "frm1701MS:txtPriorYearsNo1b",
        Gate::Joint,
        C1701MS_JOINT_COLUMN,
    ),
    (
        "1701MS",
        "frm1701MS:txtTaxPaymentsNo2b",
        Gate::Joint,
        C1701MS_JOINT_COLUMN,
    ),
    (
        "1701MS",
        "frm1701MS:txtCreditableTaxNo3b",
        Gate::Joint,
        C1701MS_JOINT_COLUMN,
    ),
    (
        "1701MS",
        "frm1701MS:txtCreditableFourthQuarterNo4b",
        Gate::Joint,
        C1701MS_JOINT_COLUMN,
    ),
    (
        "1701MS",
        "frm1701MS:txtCreditableForm2316No5b",
        Gate::Joint,
        C1701MS_JOINT_COLUMN,
    ),
    (
        "1701MS",
        "frm1701MS:txtTaxPaidPreviouslyFiledNo6b",
        Gate::Joint,
        C1701MS_JOINT_COLUMN,
    ),
    (
        "1701MS",
        "frm1701MS:txtForeignTaxCreditsNo7b",
        Gate::Joint,
        C1701MS_JOINT_COLUMN,
    ),
    (
        "1701MS",
        "frm1701MS:txtSpecialTaxCreditsNo8b",
        Gate::Joint,
        C1701MS_JOINT_COLUMN,
    ),
    (
        "1701MS",
        "frm1701MS:txtOtherCreditsPaymentsNo9b",
        Gate::Joint,
        C1701MS_JOINT_COLUMN,
    ),
    // Part IV item 16, each column's tax rate (PR 7 review): open while that
    // column's item 12 is Exempt or Special/Preferential Rate.
    (
        "1701MS",
        "frm1701MS:txtTaxRatea",
        Gate::Relief { spouse: false },
        C1701MS_RELIEF_A,
    ),
    (
        "1701MS",
        "frm1701MS:txtTaxRateb",
        Gate::Relief { spouse: true },
        C1701MS_JOINT_RELIEF,
    ),
    // Part II item 20 of each column: open once its item 19 is positive.
    (
        "1701MS",
        "frm1701MS:txtShareOfOtherNo20a",
        Gate::TaxDue { spouse: false },
        C1701MS_SHARE_A,
    ),
    (
        "1701MS",
        "frm1701MS:txtShareOfOtherNo20b",
        Gate::TaxDue { spouse: true },
        C1701MS_SHARE_B,
    ),
];

/// The gate of a box (or of a radio group whose members are gated).
fn gate_of(kind: AgentFormKind, key: &str) -> Option<Gate> {
    let code = kind.form_code();
    let own = |key: &str| {
        GATED
            .iter()
            .find(|(form, gated, _, _)| *form == code && *gated == key)
            .map(|(_, _, gate, _)| *gate)
    };
    own(key).or_else(|| {
        let entry = kind.definition().box_def(key)?;
        entry
            .members
            .iter()
            .find_map(|member| own(member))
            .filter(|_| entry.radio_group)
    })
}

/// The form's answers that open the spouse part, as printed item numbers:
/// `(civil status, spouse has income, filing status, the spouse part)`.
fn items(
    code: &str,
) -> (
    &'static str,
    Option<&'static str>,
    &'static str,
    &'static str,
) {
    match code {
        "1700" => ("15", Some("16"), "17", "Part II"),
        "1701" => ("16", Some("17"), "18", "Part IV"),
        "1701A" => ("16", Some("17"), "18", "Part V"),
        _ => ("4", None, "5", "Column B of Part I"),
    }
}

/// Whether `gate` holds on `draft`, read with the form's own rules.
fn holds(kind: AgentFormKind, draft: &Value, gate: Gate) -> bool {
    let typed = |draft: &Value| draft.clone();
    match kind.form_code() {
        "1700" => serde_json::from_value::<Form1700Draft>(typed(draft)).is_ok_and(|form| {
            let married = form.civil_status == Form1700CivilStatus::Married;
            match gate {
                Gate::Married => married,
                Gate::SpouseIncome => married && form.spouse_has_income == Some(true),
                Gate::Joint => form.is_joint(),
                Gate::TaxDue { .. } | Gate::Relief { .. } => true,
            }
        }),
        "1701" => serde_json::from_value::<Form1701Draft>(typed(draft)).is_ok_and(|form| {
            let married = form.civil_status == Some(Form1701CivilStatus::Married);
            match gate {
                Gate::Married => married,
                Gate::SpouseIncome => married && form.spouse_has_income == Some(true),
                Gate::Joint => form.spouse_column_active(),
                Gate::TaxDue { .. } | Gate::Relief { .. } => true,
            }
        }),
        "1701A" => serde_json::from_value::<Form1701ADraft>(typed(draft)).is_ok_and(|form| {
            let married = form.civil_status == Form1701ACivilStatus::Married;
            match gate {
                Gate::Married => married,
                Gate::SpouseIncome => married && form.spouse_has_income == Some(true),
                Gate::Joint => form.is_joint(),
                Gate::TaxDue { .. } | Gate::Relief { .. } => true,
            }
        }),
        "1701MS" => {
            serde_json::from_value::<Form1701MsDraft>(typed(draft)).is_ok_and(|form| match gate {
                Gate::Married | Gate::SpouseIncome => {
                    form.civil_status == Form1701MsCivilStatus::Married
                }
                Gate::Joint => form.is_joint(),
                Gate::TaxDue { spouse: false } => form.taxpayer.income_tax_due > 0.0,
                Gate::TaxDue { spouse: true } => {
                    form.is_joint() && form.spouse.income_tax_due > 0.0
                }
                Gate::Relief { spouse: false } => form.taxpayer.relief_items_open(),
                Gate::Relief { spouse: true } => form.is_joint() && form.spouse.relief_items_open(),
            })
        }
        _ => true,
    }
}

/// The refusal of a write to a closed box, in the filer's words.
fn refusal(code: &str, gate: Gate) -> String {
    let (status, income, filing, part) = items(code);
    match (gate, income) {
        (Gate::Relief { spouse }, _) => {
            let column = if spouse { "B" } else { "A" };
            format!(
                "Item 16 column {column} is the rate of an exempt or special/preferential rate income: choose Exempt or Special/Preferential Rate at item 12 column {column} first."
            )
        }
        (Gate::TaxDue { spouse }, _) => {
            let column = if spouse { "B" } else { "A" };
            format!(
                "Item 20 column {column} is for a special or preferential rate tax due: it opens once item 19 column {column} is more than zero."
            )
        }
        (Gate::Married, _) => {
            format!("This item is for a married filer: choose Married at item {status} first.")
        }
        (Gate::SpouseIncome, Some(income)) => format!(
            "Item {filing} is for a married filer whose spouse has income: choose Married at item {status} and Yes at item {income} first."
        ),
        (Gate::Joint, Some(income)) => format!(
            "{part} is for a joint return: choose Married at item {status}, Yes at item {income} and Joint Filing at item {filing} first."
        ),
        (_, None) => format!(
            "{part} is for a joint return: choose Married at item {status} and Jointly at item {filing} first."
        ),
    }
}

fn text(value: &Value) -> String {
    match value {
        Value::Number(n) => n.to_string(),
        Value::Bool(flag) => flag.to_string(),
        other => other.as_str().unwrap_or_default().trim().to_string(),
    }
}

/// A write that says nothing (blank, zero, unticked), which a closed box
/// takes: the page clears what it shows.
fn says_nothing(value: &Value) -> bool {
    let shown = text(value);
    shown.is_empty()
        || shown == "false"
        || shown.chars().all(|c| matches!(c, '0' | '.' | ',' | '-'))
}

/// `true` for a gated box whose part is closed on `draft`: rule 6 fades it
/// (whatever it shows: a legacy draft's value there is faded, not hidden).
pub(super) fn gated(kind: AgentFormKind, draft: &Value, key: &str) -> bool {
    is_mine(kind) && gate_of(kind, key).is_some_and(|gate| !holds(kind, draft, gate))
}

/// Whether rule 6 has nothing more to ask of an open gated box: its
/// condition is the gate alone (Married, a spouse with income, a joint
/// return), so once the gate holds the box is on. A box whose condition
/// needs more (a foreign tax number its Yes) is still tried the ordinary
/// way. 1701's Compensation Earner tick is set by the page's own steps, not
/// by its probed binding, so trying it the ordinary way would misread it.
pub(super) fn open_by_gate(kind: AgentFormKind, draft: &Value, key: &str) -> bool {
    if !is_mine(kind) {
        return false;
    }
    let code = kind.form_code();
    GATED
        .iter()
        .find(|(form, gated, _, _)| *form == code && *gated == key)
        .is_some_and(|(_, _, gate, condition)| {
            [
                C1700_MARRIED,
                C1700_INCOME,
                C1700_JOINT,
                C1701_MARRIED,
                C1701_INCOME,
                C1701_JOINT,
                C1701A_MARRIED,
                C1701A_INCOME,
                C1701A_JOINT,
                C1701MS_MARRIED,
                C1701MS_JOINT,
            ]
            .contains(condition)
                && holds(kind, draft, *gate)
        })
}

/// The refusal of a write to a closed spouse part (`None` for every other
/// write). The page's fill asks it before any other lock of the box.
pub(super) fn closed_refusal(
    kind: AgentFormKind,
    draft: &Value,
    key: &str,
    value: &Value,
) -> Option<String> {
    if !is_mine(kind) || says_nothing(value) {
        return None;
    }
    let gate = gate_of(kind, key)?;
    // A gate on an amount the form computes (item 19) reads the draft as
    // the batch has left it, recomputed: a batch that makes item 19
    // positive opens item 20 for its own later write.
    let computed;
    let draft = if matches!(gate, Gate::TaxDue { .. } | Gate::Relief { .. }) {
        let mut copy = draft.clone();
        let _ = agent::compute(kind, &mut copy);
        computed = copy;
        &computed
    } else {
        draft
    };
    if holds(kind, draft, gate) {
        return None;
    }
    // Column B's items 16 and 20 off a joint return say what opens column B.
    let gate = match gate {
        Gate::TaxDue { spouse: true } | Gate::Relief { spouse: true }
            if !holds(kind, draft, Gate::Joint) =>
        {
            Gate::Joint
        }
        gate => gate,
    };
    Some(refusal(kind.form_code(), gate))
}

/// Civil status and the spouse's answers first, then the spouse part, so
/// one batch can answer Married and fill it.
pub(super) fn edit_order(kind: AgentFormKind, key: &str) -> Option<usize> {
    if !is_mine(kind) {
        return None;
    }
    match gate_of(kind, key)? {
        Gate::Married => Some(1),
        Gate::SpouseIncome => Some(2),
        Gate::Joint => Some(5),
        Gate::TaxDue { .. } | Gate::Relief { .. } => Some(6),
    }
}

// ------------------------------------------------------------- TIN boxes

/// A TIN shown over four boxes, kept as one draft field: the draft path
/// and the box (0-2 the three-digit parts, 3 the branch code).
fn tin_cell(kind: AgentFormKind, key: &str) -> Option<(&'static str, usize)> {
    let part = |suffix: &str| match suffix {
        "TIN1" => Some(0),
        "TIN2" => Some(1),
        "TIN3" => Some(2),
        "BranchCode" => Some(3),
        _ => None,
    };
    match kind.form_code() {
        "1701" => {
            let tail = key.strip_prefix("frm1701:")?;
            if let Some(rest) = tail.strip_prefix("txtPg2I1") {
                return part(rest).map(|p| ("/spouse/tin", p));
            }
            // Schedule 1 items 1b and 2b, each employer's TIN.
            if let Some(rest) = tail.strip_prefix("txtPg2IShed1a_") {
                return part(rest).map(|p| ("/employers/0/employer_tin", p));
            }
            if let Some(rest) = tail.strip_prefix("txtPg2IShed2a_") {
                return part(rest).map(|p| ("/employers/1/employer_tin", p));
            }
            None
        }
        "1701A" => part(key.strip_prefix("frm1701A:txtSpouse")?).map(|p| ("/spouse/tin", p)),
        "1701MS" => {
            let rest = key
                .strip_prefix("frm1701MS:txtPg2")?
                .strip_suffix("Spouse")?;
            part(rest).map(|p| ("/spouse_info/tin", p))
        }
        _ => None,
    }
}

/// Rate boxes the probe left unbound that the field map writes from one
/// draft field: `(form, key, draft path)`. 1701MS Part IV item 16, the
/// special or preferential tax rate of each column: the field map shows
/// `special_rate` as "5.0%" (form_1701ms.rs `page_rate`), which the probe's
/// amount does not echo; the filer types it for a special-rate or exempt
/// column (Items 13-16).
const RATE_CELLS: &[(&str, &str, &str)] = &[
    ("1701MS", "frm1701MS:txtTaxRatea", "/taxpayer/special_rate"),
    ("1701MS", "frm1701MS:txtTaxRateb", "/spouse/special_rate"),
];

fn rate_cell(kind: AgentFormKind, key: &str) -> Option<&'static str> {
    RATE_CELLS
        .iter()
        .find(|(code, cell, _)| *code == kind.form_code() && *cell == key)
        .map(|(_, _, path)| *path)
}

/// The draft value a rate box takes: the percent typed ("5", "5%", "2.5").
fn rate_value(value: &Value) -> Result<Value, String> {
    let typed = text(value);
    let number = typed.trim_end_matches('%').trim().replace(',', "");
    if number.is_empty() {
        return Ok(json!(0.0));
    }
    match number.parse::<f64>() {
        Ok(rate) if (0.0..=100.0).contains(&rate) => Ok(json!(rate)),
        _ => Err(format!(
            "`{typed}` is not a rate: type the percent, as 5 or 2.5."
        )),
    }
}

/// Whether the host writes `key` itself (its "cannot be filled here" lock
/// does not apply on the page).
pub(super) fn host_writes(kind: AgentFormKind, key: &str) -> bool {
    tin_cell(kind, key).is_some() || rate_cell(kind, key).is_some()
}

/// 1701 Part VIII column A's typed items (1, 2, 4, 6, 8 and 9): the
/// official page's entry filter takes whole pesos only (`wholenumber(event,
/// true)`, as the PR 7 review read the official HTA; the tracked inventory
/// records the controls, not their key filters). A page-input limit like
/// other forms' digit limits: the box refuses a decimal or a minus sign.
/// Its own display ("1,500.00") is a whole amount and is taken.
fn whole_peso_item(kind: AgentFormKind, key: &str) -> Option<&'static str> {
    if kind.form_code() != "1701" {
        return None;
    }
    let item = key
        .strip_prefix("frm1701:txtPg4IPart8_")?
        .strip_suffix('A')?;
    ["1", "2", "4", "6", "8", "9"]
        .into_iter()
        .find(|typed| *typed == item)
}

/// The refusal of a Part VIII column A amount that is not whole pesos.
pub(super) fn whole_peso_refusal(item: &str) -> String {
    format!("Part VIII item {item}A takes whole pesos: type digits only, as 1500.")
}

/// Write `key` when this module owns it: `None` leaves it to the ordinary
/// fill.
pub(super) fn fill(
    kind: AgentFormKind,
    draft: &mut Value,
    key: &str,
    value: &Value,
) -> Option<Result<(), String>> {
    if let Some(item) = whole_peso_item(kind, key) {
        let typed = text(value);
        let fraction = typed
            .split_once('.')
            .is_some_and(|(_, cents)| cents.chars().any(|c| c != '0'));
        if typed.contains('-') || fraction {
            return Some(Err(whole_peso_refusal(item)));
        }
    }
    if let Some(path) = rate_cell(kind, key) {
        return Some((|| {
            if let Some(why) = closed_refusal(kind, draft, key, value) {
                return Err(why);
            }
            let taken = rate_value(value)?;
            let Some(slot) = draft.pointer_mut(path) else {
                return Err(format!("the draft has no {path}"));
            };
            *slot = taken;
            Ok(())
        })());
    }
    let (path, part) = tin_cell(kind, key)?;
    Some((|| {
        if let Some(why) = closed_refusal(kind, draft, key, value) {
            return Err(why);
        }
        let current = draft.pointer(path).and_then(Value::as_str).unwrap_or("");
        let tin = super::rows_1700::with_tin_part(current, part, &text(value))?;
        let Some(slot) = draft.pointer_mut(path) else {
            return Err(format!("the draft has no {path}"));
        };
        *slot = Value::String(tin);
        Ok(())
    })())
}

// ------------------------------------------------------------ display

/// What this module's boxes show: the spouse's TIN boxes as the draft holds
/// the digits while the spouse part is open (the field map writes a box only
/// once the TIN is whole), and 1700's display fixes (manual-1): the email as
/// typed, the spouse RDO and the number of attachments blank until there
/// is a value (the field map's `000` and `00` stay in the upload and print),
/// and 1701's spouse RDO blank until there is one, as its print shows it.
/// Display only.
pub(super) fn values(kind: AgentFormKind, draft: &Value, map: &mut BTreeMap<String, String>) {
    if !is_mine(kind) {
        return;
    }
    let keys: Vec<String> = kind
        .definition()
        .boxes
        .iter()
        .map(|entry| entry.key.clone())
        .filter(|key| tin_cell(kind, key).is_some())
        .collect();
    // A closed rate box (1701MS item 16) shows nothing, not the field
    // map's "0.0%" (display only; the upload keeps it).
    for (code, key, _) in RATE_CELLS {
        if *code == kind.form_code() && gated(kind, draft, key) {
            map.insert((*key).to_string(), String::new());
        }
    }
    for key in keys {
        let Some((path, part)) = tin_cell(kind, &key) else {
            continue;
        };
        if gated(kind, draft, &key) {
            continue;
        }
        let digits: String = draft
            .pointer(path)
            .and_then(Value::as_str)
            .unwrap_or("")
            .chars()
            .filter(char::is_ascii_digit)
            .collect();
        let (from, to) = [(0, 3), (3, 6), (6, 9), (9, digits.len().max(9))][part];
        let shown = digits
            .get(from.min(digits.len())..to.min(digits.len()))
            .unwrap_or("")
            .to_string();
        map.insert(key, shown);
    }
    // 1701 Schedule 1 (manual-2): one "a. Name of Employer" box per row,
    // the TPName box, shows the row's employer name whichever owner the row
    // marks (the field map writes it under the TPName or SName box by the
    // owner; the SName box is hidden).
    if kind.form_code() == "1701" {
        for (key, path) in [
            (
                "frm1701:txtPg2IShed1a_1TPName",
                "/employers/0/employer_name",
            ),
            (
                "frm1701:txtPg2IShed2a_2TPName",
                "/employers/1/employer_name",
            ),
        ] {
            if let Some(name) = draft.pointer(path).and_then(Value::as_str) {
                map.insert(key.to_string(), name.trim().to_uppercase());
            }
        }
        // Part IV item 2 (preview.0): the spouse's RDO blank until there is
        // one, as the print shows it (the upload keeps `000`).
        if let Ok(form) = serde_json::from_value::<Form1701Draft>(draft.clone()) {
            map.insert(
                "frm1701:txtPg2I2SpouseRDOCode".to_string(),
                form.spouse_rdo_shown(),
            );
        }
    }
    if kind.form_code() == "1700"
        && let Ok(form) = serde_json::from_value::<Form1700Draft>(draft.clone())
    {
        map.insert("frm1700:txtPg1I10Email".to_string(), form.email.clone());
        // Item 18, the spouse's TIN, as the draft holds the digits on a
        // joint return (the field map writes a box only once it is whole).
        if form.is_joint() {
            let digits: String = form
                .spouse
                .tin
                .chars()
                .filter(char::is_ascii_digit)
                .collect();
            for (part, key) in ["TIN1", "TIN2", "TIN3", "BranchCode"].iter().enumerate() {
                let (from, to) = [(0, 3), (3, 6), (6, 9), (9, digits.len().max(9))][part];
                let shown = digits
                    .get(from.min(digits.len())..to.min(digits.len()))
                    .unwrap_or("");
                map.insert(format!("frm1700:txtPg1I18S{key}"), shown.to_string());
            }
        }
        let rdo = if form.is_joint() {
            form.spouse.rdo_code.trim().to_string()
        } else {
            String::new()
        };
        map.insert("frm1700:txtSpouseRDOCode".to_string(), rdo);
        if form.number_of_attachments == 0 {
            map.insert(
                "frm1700:txtPg1I37NumberOfAttachments".to_string(),
                String::new(),
            );
        }
    }
}

// ------------------------------------------------------------ schema

/// Stacked address lines the sheet prints from `derived:` keys, which
/// app/forms/print-cells.json (from writer-cells joins of field keys) cannot
/// see: `(form, first line, later line, slots of each line's comb)`. 1701A
/// item 9 (R5.2): the print shapes the address over the 40- and 31-slot
/// combs (html-frozen/1701a-2018/writer-cells.json derived:address_line1
/// p1c34 and derived:address_line2 p1c35; form_1701a.rs
/// `to_print_field_map`), while the draft keeps one `registered_address`
/// that the field map writes over txtAddress (and txtAddress2 past 100
/// characters). The page shows one box over both combs, as `combs.rs`
/// pools a printed address.
const DERIVED_POOLS: &[(&str, &str, &str, [u64; 2])] = &[(
    "1701A",
    "frm1701A:txtAddress",
    "frm1701A:txtAddress2",
    [40, 31],
)];

/// Gather [`DERIVED_POOLS`] into one pool, the way `combs::apply_combs`
/// does for a pool the print cells show.
pub(super) fn mark_pools(form: &mut Value, code: &str) {
    for (pool_code, head, line, slots) in DERIVED_POOLS {
        if *pool_code != code {
            continue;
        }
        let fields = form["parts"]
            .as_array_mut()
            .into_iter()
            .flatten()
            .flat_map(|part| part["items"].as_array_mut().into_iter().flatten())
            .flat_map(|item| item["f"].as_array_mut().into_iter().flatten());
        for field in fields {
            if field["k"] == *head {
                field["pool"] = json!({
                    "keys": [head, line],
                    "slots": slots,
                    "write": [head],
                });
            } else if field["k"] == *line {
                field["t"] = Value::from("hidden");
                field["pooled"] = Value::from(*head);
            }
        }
    }
}

/// 1701 Part VIII item 6 ("Add: Special Tax Credit, if any (From Part VII
/// Item 8)"): the box the hint fills and the box it reads.
pub(super) const PART_VIII_ITEM_6: &str = "frm1701:txtPg4IPart8_6A";
pub(super) const PART_VII_ITEM_8: &str = "frm1701:txtPg4IPart7_8A";

/// Maintainer decision (1701 item 6 offers Part VII item 8) (2026-10-19): 1701
/// Part VIII item 6 stays typed (the
/// tracked inventory records no copy from Part VII item 8, ee8aced2), and
/// the page shows under it what Part VII item 8 holds, rounded half up to
/// whole pesos and saying so (P5-13), with "Use it", which
/// writes that amount into item 6 through the ordinary fill. Display and
/// page only: nothing is copied by itself and no check changes. The item
/// carries `from`: the box it reads (`k`), the box it fills (`to`) and how
/// the hint names it (`l`).
pub(super) fn mark_from(form: &mut Value, code: &str) {
    if code != "1701" {
        return;
    }
    let items = form["parts"]
        .as_array_mut()
        .into_iter()
        .flatten()
        .flat_map(|part| part["items"].as_array_mut().into_iter().flatten());
    for item in items {
        let fills = item["f"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|field| field["k"] == PART_VIII_ITEM_6);
        if fills {
            item["from"] = json!({
                "k": PART_VII_ITEM_8,
                "to": PART_VIII_ITEM_6,
                "l": "From Part VII item 8",
            });
        }
    }
}

// ------------------------------------------------------------ penalties

/// The lock of a 1701 penalty column that still holds the filer's own
/// amounts (a draft stored, or a save file imported, before the penalty
/// model: penalty_model_version 0); "computed from the due date" would be
/// wrong there. bir-core's (`agent::draft_locks::annual_individual`), which
/// locks the column on the page and refuses it to the assistant alike.
pub(crate) const KEPT_AS_ENTERED: &str = draft_locks::KEPT_AS_ENTERED;

// [`KEPT_AS_ENTERED`] for a penalty box of a column still on its typed
// amounts, `None` for every other box; and the room a 1701 or 1701Q column
// with no penalty state gets before an amount lands in it (bir-core's: the
// penalty layer's trial gives it too).
pub(super) use draft_locks::{make_room, penalty_lock};

/// Whether a penalty group's column holds the filer's own amounts: the page
/// offers "Use the computed amounts" as soon as Edit opens it.
pub(super) fn kept_as_entered(kind: AgentFormKind, keys: &[String], draft: &Value) -> bool {
    keys.first()
        .is_some_and(|key| penalty_lock(kind, draft, key).is_some())
}

/// A blank written to a column still on its typed amounts ("Use the
/// computed amounts") joins the model, so the computed amounts file.
pub(super) fn join_model(kind: AgentFormKind, draft: &mut Value, key: &str) {
    let Some((n, version)) = draft_locks::penalty_column(kind, draft, key) else {
        return;
    };
    if version >= bir_core::forms::penalty_items::PENALTY_MODEL_VERSION {
        return;
    }
    let Ok(mut form) = serde_json::from_value::<Form1701Draft>(draft.clone()) else {
        return;
    };
    let items = &Form1701Draft::penalty_declarations()[n];
    (items.state_mut)(&mut form).penalty_model_version =
        bir_core::forms::penalty_items::PENALTY_MODEL_VERSION;
    if let Ok(value) = serde_json::to_value(&form) {
        *draft = value;
    }
}

// ------------------------------------------------------------ waits

/// A box the form fills from another of its row: what it waits for while
/// blank (shown in the box). 1701 Schedule 6 items 7 and 12, the current
/// year's NOLCO row: `recompute_nolco_official` writes the year incurred and
/// the amount from item 3 when it is a loss.
pub(super) fn waits(kind: AgentFormKind) -> Map<String, Value> {
    let mut out = Map::new();
    if kind.form_code() == "1701" {
        for (key, item) in [
            ("frm1701:txtPg3IShed6_7Year", "3A"),
            ("frm1701:txtPg4IShed6_12Year", "3B"),
        ] {
            out.insert(
                key.to_string(),
                json!(format!("Filled from item {item} when it is a loss")),
            );
        }
    }
    out
}

// ------------------------------------------------------- page items

/// Each drawn box's printed item: key -> `(part name, item number)`.
fn page_items(code: &str) -> BTreeMap<String, (String, String)> {
    let mut out = BTreeMap::new();
    let Some(form) = super::schema_entry(code) else {
        return out;
    };
    for part in form["parts"].as_array().into_iter().flatten() {
        let name = part["n"].as_str().unwrap_or_default().to_string();
        for item in part["items"].as_array().into_iter().flatten() {
            let no = item["no"].as_str().unwrap_or_default().to_string();
            let keys = item["f"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|field| field["k"].as_str())
                .chain(
                    item["keys"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .flat_map(|row| row.as_array().into_iter().flatten())
                        .filter_map(Value::as_str),
                );
            for key in keys {
                out.entry(key.to_string())
                    .or_insert_with(|| (name.clone(), no.clone()));
            }
        }
    }
    out
}

/// The item numbers an official message cites ("page 1 Item 7.", "Item
/// no. 6"), in order.
fn cited_items(message: &str) -> Vec<String> {
    let lower = message.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut rest = lower.as_str();
    while let Some(at) = rest.find("item") {
        let after = &rest[at + 4..];
        let after = after.strip_prefix(" no.").unwrap_or(after);
        let after = after.trim_start();
        let number: String = after
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();
        if number.starts_with(|c: char| c.is_ascii_digit()) {
            out.push(number.to_ascii_uppercase());
        }
        rest = &rest[at + 4..];
    }
    out
}

/// An official message's boxes, kept to those under the item it cites when
/// any are (validation.0: 1701's "Please select an option for page 1 Item
/// 7." named item 21's rate boxes too, which also write the ATC). The
/// message text is unchanged; with no box under a cited item the boxes
/// stay as they were.
pub(super) fn cited_keys(kind: AgentFormKind, message: &str, keys: &mut Vec<String>) {
    if !is_mine(kind) {
        return;
    }
    let cited = cited_items(message);
    if cited.is_empty() {
        return;
    }
    let items = page_items(kind.form_code());
    let under = |key: &String| {
        items
            .get(key)
            .is_some_and(|(_, no)| cited.iter().any(|item| item.eq_ignore_ascii_case(no)))
    };
    if keys.iter().any(under) {
        keys.retain(under);
    }
}

// ------------------------------------------------------------ notices

/// Notices beside the form's own messages: nothing is changed, the filer
/// checks (as the maintainer's D5 does for 1604E's changed rows). A 1701
/// save file imported before PR 7's review kept its spouse's RDO only in
/// `spouse.rdo_code`, which the upload never read: it filed `000`, and the
/// page and the print now show what files. Such a joint draft says so on
/// Part IV item 2; its stored bytes stay as they are.
pub(super) fn notices(kind: AgentFormKind, draft: &Value) -> Vec<Value> {
    if kind.form_code() != "1701" {
        return Vec::new();
    }
    let Ok(form) = serde_json::from_value::<Form1701Draft>(draft.clone()) else {
        return Vec::new();
    };
    let imported = form.spouse.rdo_code.trim();
    if !form.spouse_column_active()
        || !form.spouse_rdo_code.trim().is_empty()
        || imported.is_empty()
        || imported == "000"
    {
        return Vec::new();
    }
    vec![json!({
        "field": "spouse_rdo_code",
        "message": spouse_rdo_notice(imported),
        "keys": ["frm1701:txtPg2I2SpouseRDOCode"],
    })]
}

/// The notice of an imported 1701 spouse RDO the filing does not carry.
pub(super) fn spouse_rdo_notice(imported: &str) -> String {
    format!("Imported spouse RDO {imported} isn't on the filing; check it.")
}

// ------------------------------------------------------------ removes

/// A host-written text box of these forms (and 1700's Schedule 1 cells):
/// what the filer typed there counts as typed.
fn host_text_box(kind: AgentFormKind, key: &str) -> bool {
    use super::rows_1700::{Cell, Field};
    tin_cell(kind, key).is_some()
        || rate_cell(kind, key).is_some()
        || matches!(
            super::rows_1700::cell(kind, key),
            Some(Cell::SpouseTin(_))
                | Some(Cell::Employer(
                    _,
                    Field::Name
                        | Field::Name2
                        | Field::Tin(_)
                        | Field::Regular
                        | Field::Flat
                        | Field::Withheld
                ))
        )
}

/// The entries the filer typed, as the page shows them: bound boxes the
/// form does not compute, the host's typed boxes, a penalty override, and
/// the spouse part's answers (a radio of a gated item).
fn typed_entries(kind: AgentFormKind, draft: &Value) -> BTreeMap<String, String> {
    let def = kind.definition();
    let code = kind.form_code();
    let Ok(map) = agent::field_map(kind, draft) else {
        return BTreeMap::new();
    };
    let typed_box = |key: &str, shown: &str| {
        let binding = def.bindings.get(key).map(|bound| &bound.binding);
        if let Some(Binding::Amount { path, .. }) = binding
            && path.contains("/penalty_overrides/")
        {
            // A 1701 column kept as entered (no penalty model yet) holds
            // the filer's own amounts.
            if penalty_lock(kind, draft, key).is_some() {
                return !says_nothing(&json!(shown));
            }
            return draft.pointer(path).is_some_and(|value| !value.is_null());
        }
        if says_nothing(&json!(shown)) {
            return false;
        }
        let computed = def.box_def(key).is_some_and(|entry| entry.computed)
            || super::schema_computed(code, key);
        let bound = matches!(
            binding,
            Some(Binding::Amount { .. } | Binding::Whole { .. } | Binding::Text { .. })
        );
        let gated_answer = matches!(binding, Some(Binding::Choice { .. }))
            && gate_of(kind, key).is_some()
            && shown == "true";
        ((bound && !computed) || host_text_box(kind, key)) || gated_answer
    };
    map.into_iter()
        .filter(|(key, shown)| typed_box(key, shown))
        .collect()
}

/// The radio group of a box: its group key, or itself.
fn group_key(kind: AgentFormKind, key: &str) -> String {
    kind.definition()
        .box_def(key)
        .and_then(|entry| entry.group.clone())
        .unwrap_or_else(|| key.to_string())
}

/// A choice: a radio, tick box or list.
fn is_answer(kind: AgentFormKind, key: &str) -> bool {
    let def = kind.definition();
    matches!(
        def.bindings.get(key).map(|bound| &bound.binding),
        Some(Binding::Choice { .. })
    ) || def.box_def(key).is_some_and(|entry| {
        ["radio", "checkbox", "select"]
            .iter()
            .any(|kind| entry.kind.contains(kind))
    })
}

/// A typed entry's printed item for a message: "item 21", "Part IV item
/// 5", or its key when the page has no number for it.
fn item_name(items: &BTreeMap<String, (String, String)>, key: &str) -> String {
    match items.get(key) {
        Some((part, no)) if !no.is_empty() => {
            let part = part.split(" · ").next().unwrap_or(part);
            if part.starts_with("Part I ") || part == "Part I" || part.starts_with("Page") {
                format!("item {no}")
            } else {
                format!("{part} item {no}")
            }
        }
        _ => key.rsplit(':').next().unwrap_or(key).to_string(),
    }
}

/// When `edits` turned `before` into `after` by removing or replacing what
/// the filer typed in other boxes, what the page asks first (`message`),
/// what the Undo toast says (`done`) and how many entries went; `None`
/// when nothing typed went. The host applies the edit either way; the page
/// keeps its old draft until the filer chooses Continue.
pub(super) fn removes(
    kind: AgentFormKind,
    before: &Value,
    after: &Value,
    edits: &Map<String, Value>,
) -> Option<Value> {
    if !is_mine(kind) {
        return None;
    }
    let had = typed_entries(kind, before);
    let has = typed_entries(kind, after);
    let edited_groups: BTreeSet<String> = edits.keys().map(|key| group_key(kind, key)).collect();
    // "Use the computed amounts" blanks a penalty group: the amounts the
    // filer typed or kept there are replaced by the computed ones, which
    // the page asks about first like any other loss.
    let to_computed = |key: &String| {
        super::penalties::path_of(kind, key).is_some()
            && edits.get(key).is_some_and(|value| {
                value.as_str().is_some_and(|text| text.trim().is_empty()) || value.is_null()
            })
    };
    let untouched = |key: &String| {
        to_computed(key)
            || (!edits.contains_key(key) && !edited_groups.contains(&group_key(kind, key)))
    };
    let shown = agent::field_map(kind, after).unwrap_or_default();
    let answered = edits.keys().any(|key| is_answer(kind, key));
    let mut cleared = Vec::new();
    let mut replaced = Vec::new();
    for (key, was) in &had {
        if !untouched(key) {
            continue;
        }
        if to_computed(key) {
            replaced.push(key.clone());
            continue;
        }
        match has.get(key) {
            None if answered
                && shown
                    .get(key)
                    .is_some_and(|now| !says_nothing(&json!(now)) && now != was) =>
            {
                replaced.push(key.clone())
            }
            None => cleared.push(key.clone()),
            Some(now) if answered && now != was => replaced.push(key.clone()),
            Some(_) => {}
        }
    }
    let lost: Vec<&String> = had
        .keys()
        .filter(|key| cleared.contains(key) || replaced.contains(key))
        .collect();
    if lost.is_empty() {
        return None;
    }
    let n = lost.len();
    let done = format!("Removed {n} entr{}", if n == 1 { "y" } else { "ies" });
    let verb = match (cleared.is_empty(), replaced.is_empty()) {
        (false, true) => "clears",
        (true, false) => "replaces",
        _ => "clears or replaces",
    };
    let page = page_items(kind.form_code());
    let mut named: Vec<String> = Vec::new();
    for key in &lost {
        let item = item_name(&page, key);
        if !named.contains(&item) {
            named.push(item);
        }
    }
    let what = if named.len() > 4 {
        format!("{} and {} more", named[..3].join(", "), named.len() - 3)
    } else if named.len() > 1 {
        format!(
            "{} and {}",
            named[..named.len() - 1].join(", "),
            named[named.len() - 1]
        )
    } else {
        named[0].clone()
    };
    Some(json!({
        "message": format!("This answer {verb} what you typed in {what}. Continue?"),
        "done": done,
        "rows": n,
    }))
}
