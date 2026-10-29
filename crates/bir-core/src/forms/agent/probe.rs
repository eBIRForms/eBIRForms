//! Derive how each official box is written into a draft, by probing.
//!
//! For every leaf of a sample draft (a string, amount, whole number, flag or
//! enum), set a recognisable value, recompute, and diff the official field
//! map. A box whose value now shows the probe ("echoes" it) is bound to that
//! leaf; a radio, checkbox or select that changes with a flag or an enum is
//! bound to the leaf values that produce each of its official values. Rows
//! past a sample's last row are probed by repeating its last row.
//!
//! Locks are derived the same way: a leaf that changes the draft's TIN, year
//! or period is the return's identity, a string that holds a taxpayer
//! profile value is the profile's, and a field named `*election*` is an
//! income-tax election.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use super::{
    AgentFormDef, AgentFormKind, Binding, BoundBox, BoxDef, BoxOption, ChoiceOption, DateFormat,
    DatePart, Lock, parse_amount, parse_error, probe_eval, probe_identity,
};

/// Boxes the rules corpus marks computed although the official page lets the
/// filer choose them. The corpus is not edited here (its digest and counts
/// are guarded by its own objective); the projection corrects it, and each
/// entry carries its evidence. Bindings are still derived by probing.
const USER_CHOSEN: &[(&str, &str)] = &[
    (
        "frm1606:txtTaxRate",
        "official BIR-Form1606.hta: a select titled \"Please select Tax Rate\" \
     (0%, 1.5%, 3%, 5%, 6%) that the page enables for the filer; form_1606.rs \
     keeps the choice in `tax_rate` (Form1606TaxRate) and checks it \
     (\"Please select the tax rate on Item 31.\")",
    ),
    (
        "frm1601EQ:ifRefund",
        "official BIR-Form1601EQ.hta: Item 30 checkbox \"To be Refunded\" \
         (onclick checkRefund), enabled for the filer when Item 30 is an \
         over-remittance; form_1601eq.rs keeps `over_remittance` and asks \
         \"Please select an Overpayment option in Part II Item 30.\"",
    ),
    (
        "frm1601EQ:ifIssueCert",
        "official BIR-Form1601EQ.hta: Item 30 checkbox \"To be issued Tax \
         Credit Certificate\" (onclick checkIssueCert), enabled on an \
         over-remittance; form_1601eq.rs `over_remittance`",
    ),
    (
        "frm1601EQ:ifCarriedOver",
        "official BIR-Form1601EQ.hta: Item 30 checkbox (onclick \
         checkCarriedOver), enabled on an over-remittance; form_1601eq.rs \
         `over_remittance`",
    ),
    (
        "frm1702RT:txtPg2Pt4I42MinimumCorporate",
        "Item 42 MCIT Due: rules/forms/1702rt-v2018c/calculations.json \
         1702rt-item42-mcit records the official computeP2Pt4I42 as \"Before \
         2020 and when IC 055 is selected, MCIT is 2% of gross; 2020+ leaves \
         the enabled value user-controlled except nonpositive net forces \
         zero\"; the page enables it once MCIT applies (enableMCITFields), and \
         form_1702rt.rs keeps it as part_iv.item_42_mcit_due, an entry \
         form_1702rt_official.rs recompute clears while the return is not \
         subject to MCIT and, for a year before 2020, computes as 2% of item \
         33 (decision P4-17)",
    ),
];
// Maintainer decision (1702EX shut rows locked like 1702MX) (2026-10-19):
// 1702EX item 49, Schedule 1 items 17e-17h and
// Schedule 2 rows 2-3 are no longer listed here. The official page keeps
// them shut at load (rules/forms/1702ex-v2018c/fixtures/
// runtime-control-inventory-v796.json: disabled), as it keeps 1702MX's
// others rows, so their amounts stay computed as fields.json marks them and
// the page locks the rows (screen_form_module rows_annual_corporate.rs).

/// Amount boxes the rules corpus marks computed although the official page
/// leaves them open for the filer to type (the corpus is read only, see
/// [`USER_CHOSEN`]). The projection marks them not computed and
/// `optional`, although the corpus says computed (a choice of
/// [`USER_CHOSEN`] becomes `conditional`).
/// 1601-FQ's corpus marks every Part II line 20-32 computed; the official
/// runtime enables 23, 24 and 28-30 at load
/// (rules/forms/1601fq-v2018/fixtures/runtime-control-inventory-v796.json:
/// `disabled: false` for txtTax23, txtTax24, txtTax28-30; the inventory
/// records 25 disabled at load, and this port's recompute keeps it only on an
/// amended return), and
/// form_1601fq.rs keeps them in `remittance_first_month`,
/// `remittance_second_month`, `tax_remitted_previous` and the penalty
/// overrides (`penalty_declaration`).
/// PR 7: 1701's corpus marks Schedule 3 items 8, 9 and 11, Schedule 4 items 3
/// and 6-13, Schedule 6's NOLCO rows 6, 7B and 9-12B and Part IV item 6
/// computed, although no rules/forms/1701-v2018/calculations.json calculation
/// outputs them and form_1701_official.rs keeps each as the filer enters it;
/// 1701MS's marks Part IV item 2 computed, an input of its own item 3
/// calculation. The probe never bound them, so the page could not type a
/// 1701 filer's sales or most of Schedule 4.
const FILER_TYPED: &[(&str, &str)] = &[
    (
        "frm1601FQ:txtTax23",
        "Item 23 Remittances Made, 1st Month: enabled at load (inventory source line 784)",
    ),
    (
        "frm1601FQ:txtTax24",
        "Item 24 2nd Month of the Quarter: enabled at load (inventory source line 803)",
    ),
    (
        "frm1601FQ:txtTax25",
        "Item 25 Tax Remitted in Return Previously Filed: the tracked inventory \
         records it disabled at load (source line 814); form_1601fq.rs `recompute` \
         zeroes it unless the return is amended",
    ),
    (
        "frm1601FQ:txtTax28",
        "Item 28 Surcharge: enabled at load (inventory source line 859)",
    ),
    (
        "frm1601FQ:txtTax29",
        "Item 29 Interest: enabled at load (inventory source line 880)",
    ),
    (
        "frm1601FQ:txtTax30",
        "Item 30 Compromise: enabled at load (inventory source line 901)",
    ),
    (
        "frm1701:txtPg2IShed3_8A",
        "Schedule 3 item 8A Sales/Revenues/Receipts/Fees: `self.typed(Schedule3, 8, party)`",
    ),
    (
        "frm1701:txtPg2IShed3_9A",
        "Schedule 3 item 9A Less: Sales Returns, Allowances and Discounts: `self.typed(Schedule3, 9, party)`",
    ),
    (
        "frm1701:txtPg2IShed3_11A",
        "Schedule 3 item 11A Less: Cost of Sales/Services: `self.typed(Schedule3, 11, party)`",
    ),
    (
        "frm1701:txtPg3IShed4_3A",
        "Schedule 4 item 3A Charitable and Other Contributions: `self.typed(Schedule4, item, party)` for items 1 to 16",
    ),
    (
        "frm1701:txtPg3IShed4_6A",
        "Schedule 4 item 6A Entertainment, Amusement and Recreation: `self.typed(Schedule4, item, party)` for items 1 to 16",
    ),
    (
        "frm1701:txtPg3IShed4_7A",
        "Schedule 4 item 7A Fringe Benefits: `self.typed(Schedule4, item, party)` for items 1 to 16",
    ),
    (
        "frm1701:txtPg3IShed4_8A",
        "Schedule 4 item 8A Interest: `self.typed(Schedule4, item, party)` for items 1 to 16",
    ),
    (
        "frm1701:txtPg3IShed4_9A",
        "Schedule 4 item 9A Losses: `self.typed(Schedule4, item, party)` for items 1 to 16",
    ),
    (
        "frm1701:txtPg3IShed4_10A",
        "Schedule 4 item 10A Pension Trusts: `self.typed(Schedule4, item, party)` for items 1 to 16",
    ),
    (
        "frm1701:txtPg3IShed4_11A",
        "Schedule 4 item 11A Rental: `self.typed(Schedule4, item, party)` for items 1 to 16",
    ),
    (
        "frm1701:txtPg3IShed4_12A",
        "Schedule 4 item 12A Research and Development: `self.typed(Schedule4, item, party)` for items 1 to 16",
    ),
    (
        "frm1701:txtPg3IShed4_13A",
        "Schedule 4 item 13A Salaries, Wages and Allowances: `self.typed(Schedule4, item, party)` for items 1 to 16",
    ),
    (
        "frm1701:txtPg2IShed3_8B",
        "Schedule 3 item 8B Sales/Revenues/Receipts/Fees: `self.typed(Schedule3, 8, party)`",
    ),
    (
        "frm1701:txtPg2IShed3_9B",
        "Schedule 3 item 9B Less: Sales Returns, Allowances and Discounts: `self.typed(Schedule3, 9, party)`",
    ),
    (
        "frm1701:txtPg2IShed3_11B",
        "Schedule 3 item 11B Less: Cost of Sales/Services: `self.typed(Schedule3, 11, party)`",
    ),
    (
        "frm1701:txtPg3IShed4_3B",
        "Schedule 4 item 3B Charitable and Other Contributions: `self.typed(Schedule4, item, party)` for items 1 to 16",
    ),
    (
        "frm1701:txtPg3IShed4_6B",
        "Schedule 4 item 6B Entertainment, Amusement and Recreation: `self.typed(Schedule4, item, party)` for items 1 to 16",
    ),
    (
        "frm1701:txtPg3IShed4_7B",
        "Schedule 4 item 7B Fringe Benefits: `self.typed(Schedule4, item, party)` for items 1 to 16",
    ),
    (
        "frm1701:txtPg3IShed4_8B",
        "Schedule 4 item 8B Interest: `self.typed(Schedule4, item, party)` for items 1 to 16",
    ),
    (
        "frm1701:txtPg3IShed4_9B",
        "Schedule 4 item 9B Losses: `self.typed(Schedule4, item, party)` for items 1 to 16",
    ),
    (
        "frm1701:txtPg3IShed4_10B",
        "Schedule 4 item 10B Pension Trusts: `self.typed(Schedule4, item, party)` for items 1 to 16",
    ),
    (
        "frm1701:txtPg3IShed4_11B",
        "Schedule 4 item 11B Rental: `self.typed(Schedule4, item, party)` for items 1 to 16",
    ),
    (
        "frm1701:txtPg3IShed4_12B",
        "Schedule 4 item 12B Research and Development: `self.typed(Schedule4, item, party)` for items 1 to 16",
    ),
    (
        "frm1701:txtPg3IShed4_13B",
        "Schedule 4 item 13B Salaries, Wages and Allowances: `self.typed(Schedule4, item, party)` for items 1 to 16",
    ),
    (
        "frm1701:txtPg3IShed6_6A",
        "Schedule 6 item 6A, the third NOLCO row's amount (row index 2): `recompute_nolco_official` keeps what the row holds (only the current-year row's amount, items 7A and 12A, is computed)",
    ),
    (
        "frm1701:txtPg3IShed6_6B",
        "Schedule 6 item 6B, its NOLCO applied in previous years: `recompute_nolco_official` keeps what the row holds (only the current-year row's amount, items 7A and 12A, is computed)",
    ),
    (
        "frm1701:txtPg3IShed6_7B",
        "Schedule 6 item 7B, the current year's row's NOLCO applied in previous years: `recompute_nolco_official` keeps what the row holds (only the current-year row's amount, items 7A and 12A, is computed)",
    ),
    (
        "frm1701:txtPg4IShed6_9A",
        "Schedule 6 item 9A, the spouse's first NOLCO row's amount: `recompute_nolco_official` keeps what the row holds (only the current-year row's amount, items 7A and 12A, is computed)",
    ),
    (
        "frm1701:txtPg4IShed6_9B",
        "Schedule 6 item 9B, its NOLCO applied in previous years: `recompute_nolco_official` keeps what the row holds (only the current-year row's amount, items 7A and 12A, is computed)",
    ),
    (
        "frm1701:txtPg4IShed6_10A",
        "Schedule 6 item 10A, the spouse's second row's amount: `recompute_nolco_official` keeps what the row holds (only the current-year row's amount, items 7A and 12A, is computed)",
    ),
    (
        "frm1701:txtPg4IShed6_10B",
        "Schedule 6 item 10B, its NOLCO applied in previous years: `recompute_nolco_official` keeps what the row holds (only the current-year row's amount, items 7A and 12A, is computed)",
    ),
    (
        "frm1701:txtPg4IShed6_11A",
        "Schedule 6 item 11A, the spouse's third row's amount: `recompute_nolco_official` keeps what the row holds (only the current-year row's amount, items 7A and 12A, is computed)",
    ),
    (
        "frm1701:txtPg4IShed6_11B",
        "Schedule 6 item 11B, its NOLCO applied in previous years: `recompute_nolco_official` keeps what the row holds (only the current-year row's amount, items 7A and 12A, is computed)",
    ),
    (
        "frm1701:txtPg4IShed6_12B",
        "Schedule 6 item 12B, the spouse's current-year row's NOLCO applied in previous years: `recompute_nolco_official` keeps what the row holds (only the current-year row's amount, items 7A and 12A, is computed)",
    ),
    (
        "frm1701:txtPg2I6TelNum",
        "Part IV item 6, the spouse's contact number: `to_official_field_map` writes spouse.contact_number there on a joint return",
    ),
    (
        "frm1701MS:txtLessNonTaxablea",
        "Part IV item 2A Less: Non-Taxable/Exempt Compensation: an input of rules/forms/1701ms-v2024/calculations.json 1701ms-part4a-item3 ('Item 3 = gross compensation - non-taxable compensation.'), a tracked rule record of the untracked official HTA runtime; form_1701ms.rs keeps non_taxable_compensation as entered while Schedule A (Part IV.A) is open and clears it when Schedule A closes",
    ),
    (
        "frm1701MS:txtLessNonTaxableb",
        "Part IV item 2B Less: Non-Taxable/Exempt Compensation: an input of rules/forms/1701ms-v2024/calculations.json 1701ms-part4a-item3 ('Item 3 = gross compensation - non-taxable compensation.'), a tracked rule record of the untracked official HTA runtime; form_1701ms.rs keeps non_taxable_compensation as entered while Schedule A (Part IV.A) is open and clears it when Schedule A closes",
    ),
];

const EXCESS_MCIT_1702RT: &str = "1702RT Schedule IV column C (Excess MCIT over \
     Normal Income Tax), rows 2 and 3: form_1702rt_official.rs recompute sets \
     each row's excess_mcit to MCIT less normal income tax (floored at zero), \
     and the official page disables the box at load \
     (rules/forms/1702rt-v2018c/fixtures/runtime-control-inventory-v796.json \
     source lines 3188 and 3215, like row 1's at 3161); the samples leave the \
     normal income tax at zero, so probing saw the box echo the MCIT and bound \
     it to /schedule_4/rows/N/mcit";

/// Profile boxes the rules corpus marks computed: the official page fills
/// them from the taxpayer's profile and shuts them at load, exactly as it
/// does the profile boxes beside them that the corpus leaves uncomputed, so
/// the projection treats them as those (still `required` as the corpus
/// says). Their bindings, and their profile lock, are derived by probing.
const PROFILE_NOT_COMPUTED: &[(&str, &str)] = &[(
    "frm1702EX:txtPg1Pt1I11ContactNumber",
    "Item 11 Contact Number: rules/forms/1702ex-v2018c/fields.json marks it \
     computed, but rules/forms/1702ex-v2018c/fixtures/\
     runtime-control-inventory-v796.json records it disabled at load (source \
     line 697) like the item 8, 9, 9A and 12 profile boxes (source lines \
     592-711), and form_1702ex.rs writes it from the draft's contact_number, \
     which new_from_profile fills from the profile's phone",
)];

/// Boxes whose value only coincides with the sample profile's: the recorded
/// samples put the property and the spouse in the filer's own RDO (039), so
/// the probe would lock them as the profile's. They are the filer's answers.
const NOT_PROFILE: &[(&str, &str)] = &[
    (
        "frm1606:txtRDOCode16A",
        "Item 16A: the RDO where the property is",
    ),
    (
        "frm1706:txtRDOCode14A",
        "Item 14A: the RDO where the property is",
    ),
    (
        "frm1701:txtPg2I2SpouseRDOCode",
        "Part IV Item 2: the spouse's RDO",
    ),
    ("frm1701A:txtSpouseRDOCode", "the spouse's RDO"),
    ("frm1701MS:txtRDOCodeb", "the spouse's RDO"),
    ("frm1700:txtSpouseRDOCode", "the spouse's RDO"),
    ("frm1707:txtPg1I6SellerName0", "Item 6: the seller's name"),
    (
        "frm2000:sched4:txtNumberTo0",
        "Schedule 4: a document's last number",
    ),
    (
        "frm2000:sched4:txtNumberTo1",
        "Schedule 4: a document's last number",
    ),
    (
        "frm2000:sched4:txtNumberTo2",
        "Schedule 4: a document's last number",
    ),
    (
        "frm2000:sched4:txtNumberTo3",
        "Schedule 4: a document's last number",
    ),
    (
        "frm2000:sched4:txtNumberTo4",
        "Schedule 4: a document's last number",
    ),
    (
        "frm2000:sched4:txtNumberTo5",
        "Schedule 4: a document's last number",
    ),
    (
        "frm2000:sched4:txtNumberTo6",
        "Schedule 4: a document's last number",
    ),
    (
        "frm2000:sched4:txtNumberTo7",
        "Schedule 4: a document's last number",
    ),
    (
        "frm2000:sched4:txtNumberTo8",
        "Schedule 4: a document's last number",
    ),
    (
        "frm2000:sched4:txtNumberTo9",
        "Schedule 4: a document's last number",
    ),
    (
        "frm2000:sched4:txtNumberTo10",
        "Schedule 4: a document's last number",
    ),
    (
        "frm2000:sched4:txtNumberTo11",
        "Schedule 4: a document's last number",
    ),
];

/// Boxes the rules corpus lists as entries although the form computes them
/// (the opposite of [`USER_CHOSEN`]): the projection marks them computed, so
/// they are never bound to a draft field. The field map and the upload are
/// unchanged.
const COMPUTED_BY_FORM: &[(&str, &str)] = &[
    (
        "frm2550qv2024:penalties",
        "Item 25 Total Penalties (Sum of Items 22 to 24): form_2550q.rs recompute \
         sets part_ii.item_25_total_penalties to the sum of Items 22-24; the \
         samples leave Items 23 and 24 unset, so probing saw it move with the \
         surcharge alone and bound it to /part_ii/item_22_surcharge",
    ),
    ("frm1604e:txtSched1TotRemAmt1", TOTAL_REMITTED_1604E),
    ("frm1604e:txtSched1TotRemAmt2", TOTAL_REMITTED_1604E),
    ("frm1604e:txtSched1TotRemAmt3", TOTAL_REMITTED_1604E),
    ("frm1604e:txtSched1TotRemAmt4", TOTAL_REMITTED_1604E),
    ("frm1604e:txtSched1TotRemAmtTtl", TOTAL_REMITTED_1604E),
    ("frm1604e:txtSched2TotRemAmt1", TOTAL_REMITTED_1604E),
    ("frm1604e:txtSched2TotRemAmt2", TOTAL_REMITTED_1604E),
    ("frm1604e:txtSched2TotRemAmt3", TOTAL_REMITTED_1604E),
    ("frm1604e:txtSched2TotRemAmt4", TOTAL_REMITTED_1604E),
    ("frm1604e:txtSched2TotRemAmt5", TOTAL_REMITTED_1604E),
    ("frm1604e:txtSched2TotRemAmt6", TOTAL_REMITTED_1604E),
    ("frm1604e:txtSched2TotRemAmt7", TOTAL_REMITTED_1604E),
    ("frm1604e:txtSched2TotRemAmt8", TOTAL_REMITTED_1604E),
    ("frm1604e:txtSched2TotRemAmt9", TOTAL_REMITTED_1604E),
    ("frm1604e:txtSched2TotRemAmt10", TOTAL_REMITTED_1604E),
    ("frm1604e:txtSched2TotRemAmt11", TOTAL_REMITTED_1604E),
    ("frm1604e:txtSched2TotRemAmt12", TOTAL_REMITTED_1604E),
    ("frm1604e:txtSched2TotRemAmtTtl", TOTAL_REMITTED_1604E),
    ("frm1702RT:txtPg4Sc4I2C4", EXCESS_MCIT_1702RT),
    ("frm1702RT:txtPg4Sc4I3C4", EXCESS_MCIT_1702RT),
    // PR 7: 1701's totals. form_1701_official.rs `official_recompute` puts
    // each from the items it sums, so a value typed there was overwritten;
    // the probe, which saw them move with one of those items, had bound them
    // to that item (16A wrote Schedule 4 item 1, 14A Schedule 5 item 1's
    // amount, 15A and 8D the first NOLCO row's current-year application,
    // Part IX 10B item 6B).
    (
        "frm1701:txtPg2IShed3_14A",
        "Schedule 3 item 14A Special Allowable Itemized Deductions (From Schedule 5 \
         Item 3): `self.put(Schedule3, 14, party, i14)` with i14 the Schedule 5 total",
    ),
    (
        "frm1701:txtPg2IShed3_15A",
        "Schedule 3 item 15A NOLCO (From Schedule 6 Item 8): \
         `self.put(Schedule3, 15, party, i15)` with i15 the Schedule 6 total",
    ),
    (
        "frm1701:txtPg2IShed3_16A",
        "Schedule 3 item 16A Total Allowable Itemized Deductions (Sum of Items 13 to \
         15): `self.put(Schedule3, 16, party, item_16)`",
    ),
    (
        "frm1701:txtPg2IShed3_22B",
        "Schedule 3 item 22B Total Other Non-Operating Income (Sum of Items 19 to 21): \
         `self.put(Schedule3, 22, party, item_22)`",
    ),
    (
        "frm1701:txtPg3IShed6_8D",
        "Schedule 6 item 8D Total NOLCO - Taxpayer/Filer (Sum of Items 4D to 7D): \
         `recompute_nolco_official` sets schedule_6_total_taxpayer to the sum",
    ),
    (
        "frm1701:txtPg4IPart9_10A",
        "Part IX item 10A Total (Sum of Items 6 to 9): `self.put(PartIx, 10, party, ix_10)`",
    ),
    (
        "frm1701:txtPg4IPart8_3A",
        "Part VIII item 3A Sub-Total - Tax Relief (Sum of Items 1 and 2): `self.put(PartViii, 3, party, ..)`; \
         an output of rules/forms/1701-v2018/calculations.json 1701-part8-running-totals, \
         its operator the printed caption's",
    ),
    (
        "frm1701:txtPg4IPart8_5A",
        "Part VIII item 5A Tax Relief Availment Before Special Tax Credit (Item 3 Less Item 4): `self.put(PartViii, 5, party, ..)`; \
         an output of rules/forms/1701-v2018/calculations.json 1701-part8-running-totals, \
         its operator the printed caption's",
    ),
    (
        "frm1701:txtPg4IPart8_7A",
        "Part VIII item 7A Total Tax Relief Availment - SPECIAL (Sum of Items 5 and 6): `self.put(PartViii, 7, party, ..)`; \
         an output of rules/forms/1701-v2018/calculations.json 1701-part8-running-totals, \
         its operator the printed caption's",
    ),
    (
        "frm1701:txtPg4IPart8_10A",
        "Part VIII item 10A Total Tax Relief Availment - EXEMPT (Sum of Items 8 and 9): `self.put(PartViii, 10, party, ..)`; \
         an output of rules/forms/1701-v2018/calculations.json 1701-part8-running-totals, \
         its operator the printed caption's",
    ),
    (
        "frm1701:txtPg4IPart9_10B",
        "Part IX item 10B Total (Sum of Items 6 to 9): `self.put(PartIx, 10, party, ix_10)`",
    ),
];

const TOTAL_REMITTED_1604E: &str = "1604E Part II Total Amount Remitted: form_1604e.rs \
     compute_schedule (computeSched1/computeSched2) sets each row's \
     total_remitted to taxes_withheld + penalties and the TOTAL row's to the \
     sum of the rows; the sample leaves most rows' taxes at zero, so probing \
     saw a row's total move with its penalties alone and bound it to \
     /scheduleN/k/penalties";

/// Part II date boxes the samples date only in part (PR 6, 1604C/E/F): a
/// row path a second sample echoes as a whole MM/DD/YYYY date is that date,
/// though the first sample, which leaves the row blank, echoed plain text.
/// Each entry is a box key stem; the rest of the key is the row number.
const DATED_ROWS_KEEP_THE_DATE: &[&str] = &[
    "frm1604c:txtSched1Date",
    "frm1604e:txtSched1RemDate",
    "frm1604e:txtSched2RemDate",
    "frm1604f:txtSched1Date",
    "frm1604f:txtSched2Date",
    "frm1604f:txtSched3Date",
];

fn keeps_the_date(key: &str) -> bool {
    DATED_ROWS_KEEP_THE_DATE.iter().any(|stem| {
        key.strip_prefix(stem)
            .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
    })
}

/// Boxes `fields.json` keys only as an occurrence of an official control
/// (`X#occurrence-1`) that it lists under no key of its own, while the app's
/// field map writes the control once, under `X`: `(form code, box key,
/// evidence)`. The probe reads such a box as `X`, so it binds to the draft
/// field `X` shows; the field map, the upload and the print are unchanged.
pub(crate) const FIRST_OCCURRENCE: &[(&str, &str, &str)] = &[
    (
        "2551Q",
        "txtEmail#occurrence-1",
        "Item 11 Email Address: the HTA has two txtEmail inputs (item 11 and a \
     hidden page-2 copy), which fields.json lists as occurrence-1 and \
     occurrence-2; the upload writes the one value as txtEmail \
     (tests/sample-payloads/2551Q-122025Q1.official.xml) and the sheet \
     prints it once (html-frozen/2551q-2018/writer-cells.json join txtEmail)",
    ),
    (
        "1702Q",
        "frm1702q:txtTelNum#occurrence-1",
        "Item 10 Contact Number (PR 5): rules/forms/1702q-v2018c/fields.json lists \
         the two txtTelNum controls as occurrence-1 (item 10) and occurrence-2 \
         (\"Hidden duplicate profile contact number\"); the upload writes the one \
         value under txtTelNum for both (tests/sample-payloads/1702Q-2025Q2.official.xml) \
         and the sheet prints it once (html-frozen/1702q-2018/writer-cells.json join \
         frm1702q:txtTelNum, \"10 Contact Number\")",
    ),
];

/// Boxes the taxpayer profile fills although the samples put another value
/// there, so the probe cannot see the profile's string in them. Each entry
/// carries its evidence.
const PROFILE_FACT: &[(&str, &str)] = &[(
    "frm0619E:txtAddress",
    "Item 10, the registered address: Form0619EDraft::new_from_profile copies \
     profile.registered_address, as 0619F's does (its txtAddress is a profile \
     box); the 0619E sample (tests/sample_payloads/form_0619e.rs) replaces it \
     with a longer address to reach the second official line",
)];

/// Printed boxes the official program injects at run time, so `fields.json`
/// (read only) lists no box for them, although its validations name them and
/// the app's field map writes them: each is supplemented as a box of its
/// printed item, and its binding is still derived by probing. Each entry:
/// `(key, item, printed caption, evidence)`.
const PRINTED_NOT_IN_CORPUS: &[(&str, &str, &str, &str)] = &[
    (
        "frm2000:txtRDOCode",
        "5",
        "RDO Code",
        "Item 5 RDO Code: rules/forms/2000-v2018/validations.json 2000-val-006-rdo \
     names frm2000:txtRDOCode (\"Please enter a valid RDO Code on Item 5.\"), \
     html-frozen/2000-dst-2018/writer-cells.json joins it at \"5 RDO Code\", and \
     form_2000.rs writes it from the draft's rdo_code; fields.json has no row for it",
    ),
    (
        "frm1707:rdoPg1Pt1I5RDO",
        "5",
        "RDO Code of the Seller",
        "Item 5 RDO Code of the Seller (PR 9): rules/forms/1707-v2021/validations.json \
     1707-validate-009-rdo names frm1707:rdoPg1Pt1I5RDO (\"Please enter a valid RDO \
     Code on Page 1 Item 5.\", a tracked rule record of the untracked official HTA \
     runtime), html-frozen/1707-2021/writer-cells.json joins it at \"5 RDO Code of \
     the Seller\", and form_1707.rs writes it from the draft's rdo_code; fields.json \
     has no row for it",
    ),
];

/// Radios this app draws that the official page does not, so `fields.json`
/// has no row. The field map still writes them, and the probe binds them
/// beside the official radios that set the same draft fields.
///
/// 2551Q Item 13 Not applicable (`frm2551Qv2018:taxRateNA`) is the third
/// answer beside Graduated and 8%. `Form2551QDraft`'s field map writes it
/// from `item_13_election == not_applicable`. It is not an upload key
/// (`to_bir_field_map` stays taxRate1 and taxRate2).
pub fn supplement_app_radios(kind: AgentFormKind, boxes: &mut Vec<BoxDef>) {
    if kind != AgentFormKind::Form2551Q {
        return;
    }
    let key = "frm2551Qv2018:taxRateNA";
    if boxes.iter().any(|entry| entry.key == key) {
        return;
    }
    let radio = BoxDef {
        key: key.to_string(),
        label: "13 Not applicable".to_string(),
        item: "13".to_string(),
        kind: "radio".to_string(),
        computed: false,
        required: "optional".to_string(),
        options: vec![
            BoxOption {
                value: "true".to_string(),
                label: String::new(),
            },
            BoxOption {
                value: "false".to_string(),
                label: String::new(),
            },
        ],
        layout_only: false,
        group: None,
        radio_group: false,
        members: Vec::new(),
        choice: String::new(),
        choice_value: String::new(),
        family: None,
    };
    let at = boxes
        .iter()
        .position(|entry| entry.key == "frm2551Qv2018:taxRate2")
        .map_or(boxes.len(), |index| index + 1);
    boxes.insert(at, radio);
}

/// Text boxes the form's own page upper-cases when another field is typed,
/// so the probe sees them change with that field and would call them a
/// combination: 1701MS's `capital(this, event)` on Part V items 7 and 9
/// upper-cases every text control (form_1701ms.rs `part_v_capitalizes`).
/// They are bound to the one path they echo; the field map, the upload and
/// the print are unchanged.
const CASE_FOLLOWS_ANOTHER_FIELD: &[(&str, &str)] = &[
    (
        "frm1701MS:txtEmaila",
        "Item 9A Email Address: the field map writes /email, upper-cased once \
         Part V items 7 or 9 are typed (part_v_capitalizes)",
    ),
    (
        "frm1701MS:txtEmailb",
        "Item 9B the spouse's Email Address: the field map writes \
         /spouse_info/email on a joint return, upper-cased as item 9A",
    ),
];

/// Choice options whose probed draft value is overruled, with evidence:
/// `(key, option value, draft path, value it sets, evidence)`. 1701MS's ATC
/// II012 is "Business Income-Graduated IT Rates" (html-frozen/1701ms-2024/
/// guide.html ATC table; rules/forms/1701ms-v2024/fields.json lists it among
/// the item 18 options), but the ATC is the same code for
/// a business taxpayer on special rates, so a seed on special rates showed
/// the probe II012 with `tax_option = special` and it bound the code to
/// special: a special rate left at 0 then files the business tax as 0. The
/// code sets graduated, as II013 and II014 do; item 12 still chooses
/// special (PR 7 review).
///
/// `(form code, key, option value, draft path, value it sets, evidence)`:
/// scoped by form, so another form's `taxCodeDropdown` never inherits it.
///
/// PR 9: 1606's and 1706's item 11 Yes (rdTreaty) is
/// "yes, with nothing picked from the list yet" (`YesUnspecified`, the
/// official `enableSelTreaty` opens the list blank), but the samples answer
/// it with special law, so the probe bound Yes to `special_law`: Yes alone
/// then filed special-law relief and the official "Please select a tax
/// relief" check could never fire. The list still picks the relief.
const CHOICE_OPTION_SETS: &[(&str, &str, &str, &str, &str, &str)] = &[
    (
        "1701MS",
        "taxCodeDropdown",
        "II012",
        "/taxpayer/tax_option",
        "graduated",
        "1701MS item 18 column A: II012 Business Income - Graduated IT Rates",
    ),
    (
        "1701MS",
        "taxCodeDropdownSpouse",
        "II012",
        "/spouse/tax_option",
        "graduated",
        "1701MS item 18 column B: II012 Business Income - Graduated IT Rates",
    ),
    (
        "1606",
        "frm1606:rdTreaty",
        "1",
        "/tax_relief",
        "yes_unspecified",
        "1606 item 11 Yes: form_1606.rs Form1606TaxRelief::YesUnspecified",
    ),
    (
        "1606",
        "frm1606:rdTreaty:_1",
        "true",
        "/tax_relief",
        "yes_unspecified",
        "1606 item 11 Yes: form_1606.rs Form1606TaxRelief::YesUnspecified",
    ),
    (
        "1606",
        "frm1606:rdTreaty:_2",
        "false",
        "/tax_relief",
        "yes_unspecified",
        "1606 item 11 No unticked is Yes: Form1606TaxRelief::YesUnspecified",
    ),
    (
        "1706",
        "frm1706:rdTreaty",
        "1",
        "/tax_relief",
        "yes_unspecified",
        "1706 item 11 Yes: form_1706.rs Form1706TaxRelief::YesUnspecified",
    ),
    (
        "1706",
        "frm1706:rdTreaty:_1",
        "true",
        "/tax_relief",
        "yes_unspecified",
        "1706 item 11 Yes: form_1706.rs Form1706TaxRelief::YesUnspecified",
    ),
    (
        "1706",
        "frm1706:rdTreaty:_2",
        "false",
        "/tax_relief",
        "yes_unspecified",
        "1706 item 11 No unticked is Yes: Form1706TaxRelief::YesUnspecified",
    ),
    // PR 14 (review F9): 2200S item 17 Other Similar Schemes is a kind of
    // prepayment — its text is kept only "with item 17 ticked on a
    // prepayment" and `compute` clears `other_scheme` unless the manner is
    // a prepayment, so the pick sets both (the boolean half is in
    // CHOICE_OPTION_BOOL_SETS).
    (
        "2200S",
        "frm2200S:chkPymntManner",
        "3",
        "/manner",
        "prepayment",
        "2200S item 17 Other Similar Schemes: a kind of prepayment (its text needs item 16 = Prepayment and item 17)",
    ),
    (
        "2200S",
        "frm2200S:chkPymntManner_3",
        "true",
        "/manner",
        "prepayment",
        "2200S item 17 Other Similar Schemes: a kind of prepayment (its text needs item 16 = Prepayment and item 17)",
    ),
];

/// Choices whose option also sets a boolean draft field, with evidence:
/// `(form code, key, option value, path, set, evidence)`. Scoped by form
/// and key like [`CHOICE_OPTION_SETS`], which carries strings; these sets
/// are booleans.
///
/// PR 14 (review F9): 2200S's items 15, 16 and 17 are one three-member
/// official payment-manner group — item 17 Other Similar Schemes is a
/// kind of prepayment (its text is kept only "with item 17 ticked on a
/// prepayment", and `compute` clears `other_scheme` unless the manner is
/// a prepayment). The probe ties each member to the one path its field
/// map write moves, so picking item 17 beside item 15 could file Actual
/// Removal with `other_scheme` — a pair the form cannot hold — and
/// picking item 15 or 16 left a stale `other_scheme` tick. Each manner
/// pick now states both paths: 15/16 clear `other_scheme`, 17 picks a
/// prepayment with `other_scheme`.
const CHOICE_OPTION_BOOL_SETS: &[(&str, &str, &str, &str, bool, &str)] = &[
    (
        "2200S",
        "frm2200S:chkPymntManner",
        "1",
        "/other_scheme",
        false,
        "2200S item 15 Actual Removal: form_2200s.rs `compute` clears other_scheme unless manner is a prepayment",
    ),
    (
        "2200S",
        "frm2200S:chkPymntManner",
        "2",
        "/other_scheme",
        false,
        "2200S item 16 Prepayment: prepayment alone is not Other Similar Schemes (item 17)",
    ),
    (
        "2200S",
        "frm2200S:chkPymntManner_1",
        "true",
        "/other_scheme",
        false,
        "2200S item 15 Actual Removal: form_2200s.rs `compute` clears other_scheme unless manner is a prepayment",
    ),
    (
        "2200S",
        "frm2200S:chkPymntManner_2",
        "true",
        "/other_scheme",
        false,
        "2200S item 16 Prepayment: prepayment alone is not Other Similar Schemes (item 17)",
    ),
];

/// Choices whose options may set only draft fields under a prefix, with
/// evidence: `(form code, key, path prefix, evidence)`. 1701MS item 18 column B, the
/// spouse's ATC, is open only on a joint return (form_1701ms.rs `recompute`
/// resets the spouse column off one, as the official marriedJointly /
/// toggleLine5 do), so every seed that answers it is a joint one and the
/// probe tied each code to `/civil_status = married` and `/filing =
/// jointly`, and the blank option to `single`: clearing the spouse's ATC
/// made the return Single and wiped column B. Its options set the spouse's
/// own fields only (PR 7 review).
const CHOICE_PATHS_WITHIN: &[(&str, &str, &str, &str)] = &[(
    "1701MS",
    "taxCodeDropdownSpouse",
    "/spouse/",
    "1701MS item 18 column B sets the spouse's source and tax option only",
)];

#[cfg(test)]
mod choice_list_tests {
    use super::{CHOICE_OPTION_BOOL_SETS, CHOICE_OPTION_SETS, CHOICE_PATHS_WITHIN};

    /// Every scoped entry names a reviewed form (1701MS, PR 7; 1606 and
    /// 1706, PR 9; 2200S, PR 14) and a key that form's pin binds, so no
    /// other form's same-named box is touched.
    #[test]
    fn choice_overrides_are_scoped_to_their_form() {
        let pin = |form: &str| match form {
            "1701MS" => include_str!("../../../data/agent-forms/1701ms-v2024.json"),
            "1606" => include_str!("../../../data/agent-forms/1606-v2018.json"),
            "1706" => include_str!("../../../data/agent-forms/1706-v2018.json"),
            "2200S" => include_str!("../../../data/agent-forms/2200s-v2018.json"),
            other => panic!("{other} is not a reviewed form"),
        };
        let keys = CHOICE_OPTION_SETS
            .iter()
            .map(|(form, key, ..)| (*form, *key))
            .chain(
                CHOICE_OPTION_BOOL_SETS
                    .iter()
                    .map(|(form, key, ..)| (*form, *key)),
            )
            .chain(
                CHOICE_PATHS_WITHIN
                    .iter()
                    .map(|(form, key, ..)| (*form, *key)),
            );
        for (form, key) in keys {
            assert!(
                pin(form).contains(&format!("\"{key}\": {{")),
                "{form} {key}"
            );
        }
    }
}

/// Boxes the official page keeps disabled from load, with no tracked
/// source that enables them: the field map writes their page default and
/// the page shows it shut with this reason. `(form code, key)`. 1701 Part
/// VIII column B: rules/forms/1701-v2018/fixtures/runtime-control-inventory-v796.json
/// records all ten `disabled: true` (source lines 3363, 3373, 3379, 3389,
/// 3395, 3405, 3411, 3431, 3441 and 3447), unlike Part VII and Part IX
/// column B; form_1701_official.rs files them 0.00 (PR 7 review).
/// The official ATC popup's tick boxes, with evidence: `(form code, key
/// prefix, reason)`. Each slot ticks one ATC of the list the category of
/// withholding agent offers, and the field map writes the box from the
/// Part II rows. The number of slots follows the category, so the probe tied
/// slots to the category field: a direct fill of a slot wrote the
/// category, never the ATC, and the box still showed unticked (PR 14 review
/// F10, F12). They are left unbound; the reason keeps the probe's own
/// evidence for each slot.
///
/// The desktop assistant fills through `agent::fill`, so it gets the reason
/// as a refusal and chooses a row's ATC in the row instead. The bir-shell
/// page host writes 1600PT's and 1600VT's slots itself (rows_monthly.rs
/// `Cell::Slot`); 1601FQ's stay refused there too.
const POPUP_SLOTS: &[(&str, &str, &str)] = &[
    (
        "1600PT",
        "AtcCode",
        "the ATC popup's tick box (item 10's list): a Part II row's ATC is chosen in the row (txtAtcCd<n>)",
    ),
    (
        "1600VT",
        "AtcCode",
        "the ATC popup's tick box (item 10's list): a Part II row's ATC is chosen in the row (txtAtcCd<n>)",
    ),
    (
        "1601FQ",
        "AtcCd",
        "the ATC popup's tick box (the category's list): a Part II row's ATC is chosen in the row (txtAtcCode<n>)",
    ),
];

/// Radios whose fields.json item is not the one their official name group
/// answers, with evidence: `(form code, key, item, evidence)`. They are
/// grouped as that item's radios.
const RADIO_ITEM: &[(&str, &str, &str, &str)] = &[(
    "2200C",
    "frm2200C:rdoPg1Pt2MOPPaymentActual",
    "",
    "rules/forms/2200c-v2018/fixtures/runtime-control-inventory-v796.json: \
     rdoPg1Pt2MOPPaymentActual, rdoPg1Pt2MOPPrepayment and rdoPg1Pt2MOPOther are \
     one radio name group (frm2200C:rdoPg1Pt2MOP); fields.json lists the first under Part V",
)];

const SHUT_ON_OFFICIAL_PAGE: &[(&str, &str)] = &[
    ("1701", "frm1701:txtPg4IPart8_1B"),
    ("1701", "frm1701:txtPg4IPart8_2B"),
    ("1701", "frm1701:txtPg4IPart8_3B"),
    ("1701", "frm1701:txtPg4IPart8_4B"),
    ("1701", "frm1701:txtPg4IPart8_5B"),
    ("1701", "frm1701:txtPg4IPart8_6B"),
    ("1701", "frm1701:txtPg4IPart8_7B"),
    ("1701", "frm1701:txtPg4IPart8_8B"),
    ("1701", "frm1701:txtPg4IPart8_9B"),
    ("1701", "frm1701:txtPg4IPart8_10B"),
];

/// The unmapped reason of a [`SHUT_ON_OFFICIAL_PAGE`] box.
pub const SHUT_REASON: &str = "shut on the official page from load (its runtime-control inventory: disabled), filed as its 0.00";

/// Amounts the filer types that another field only clears. The probe sees
/// the box move with that other field and would call it a combination; here
/// the other field closes a gate (no overpayment left) and `compute` empties
/// the entry. They are bound to the one path they echo.
const GATED_ENTRY: &[(&str, &str)] = &[
    (
        "frm1701MS:txtToBeRefunded1",
        "Item 31 \"To be refunded\" amount: the filer types it once the box \
         is ticked (official validate: \"The total amount in Item 31 must \
         equal the sum of Item 27A and Item 27B\"); form_1701ms.rs \
         `compute` clears it when a penalty such as the compromise ends the \
         overpayment",
    ),
    (
        "frm1701MS:txtToBeIssued1",
        "Item 31 \"To be issued a Tax Credit Certificate\" amount: typed by \
         the filer; cleared by `compute` with its box when the overpayment \
         ends",
    ),
    (
        "frm1701MS:txtToBeCarried1",
        "Item 31 \"To be carried over\" amount: typed by the filer; cleared \
         by `compute` with its box when the overpayment ends",
    ),
    // PR 5: 1702Q's schedule entries. form_1702q.rs `recompute` clears
    // Schedules 1, 2 and 4 with no ATC at Item 5 (tracked rule record
    // rules/forms/1702q-v2018c/validations.json 1702q-change-007 of the
    // untracked official HTA runtime) and Schedule 3 while `mcit_open` does
    // not hold (port behaviour), which the probe sees as a combination with
    // /atc. Schedule 2 item 6 is typed under itemized deductions and
    // overwritten under OSD (port behaviour).
    ("frm1702q:Sched1:txtTax1B", GATED_1702Q_SCHEDULE),
    ("frm1702q:Sched1:txtTax2B", GATED_1702Q_SCHEDULE),
    ("frm1702q:Sched1:txtTax4B", GATED_1702Q_SCHEDULE),
    ("frm1702q:Sched1:txtTax6B", GATED_1702Q_SCHEDULE),
    ("frm1702q:Sched1:txtTax8B", GATED_1702Q_SCHEDULE),
    ("frm1702q:Sched1:txtTax10B", GATED_1702Q_SCHEDULE),
    ("frm1702q:Sched2:txtTax1", GATED_1702Q_SCHEDULE),
    ("frm1702q:Sched2:txtTax2", GATED_1702Q_SCHEDULE),
    ("frm1702q:Sched2:txtTax4", GATED_1702Q_SCHEDULE),
    ("frm1702q:Sched2:txtTax6", GATED_1702Q_SCHEDULE),
    ("frm1702q:Sched2:txtTax8", GATED_1702Q_SCHEDULE),
    ("frm1702q:Sched3:txtTax1", GATED_1702Q_SCHEDULE),
    ("frm1702q:Sched3:txtTax2", GATED_1702Q_SCHEDULE),
    ("frm1702q:Sched3:txtTax3", GATED_1702Q_SCHEDULE),
    ("frm1702q:Sched4:txtTax1", GATED_1702Q_SCHEDULE),
    ("frm1702q:Sched4:txtTax2", GATED_1702Q_SCHEDULE),
    ("frm1702q:Sched4:txtTax3", GATED_1702Q_SCHEDULE),
    ("frm1702q:Sched4:txtTax4", GATED_1702Q_SCHEDULE),
    ("frm1702q:Sched4:txtTax5", GATED_1702Q_SCHEDULE),
    ("frm1702q:Sched4:txtOthrTxCrdts0", GATED_1702Q_SCHEDULE),
    ("frm1702q:Sched4:txtOthrTxCrdts1", GATED_1702Q_SCHEDULE),
    ("frm1702q:Sched4:txtOthrTxCrdtAmnt0", GATED_1702Q_SCHEDULE),
    ("frm1702q:Sched4:txtOthrTxCrdtAmnt1", GATED_1702Q_SCHEDULE),
    (
        "frm1702RT:txtPg4Sc4I1C1",
        "Schedule IV item 1 Year: typed by the filer like items 2 and 3; \
         form_1702rt_official.rs recompute clears every Schedule IV row \
         while the return is not subject to MCIT (enableMCITFields(false)), \
         which the taxable year decides",
    ),
    (
        "frm1702RT:txtPg4Sc4I1C2",
        "Schedule IV item 1 A) Normal Income Tax as Adjusted: typed by the \
         filer; cleared with its row while the return is not subject to MCIT",
    ),
    (
        "frm1702RT:txtPg4Sc4I1C3",
        "Schedule IV item 1 B) MCIT: typed by the filer; cleared with its \
         row while the return is not subject to MCIT",
    ),
    ("frm1702MX:txtPg7Sc9I1", MX_SCHEDULE_9_ROW_1),
    ("frm1702MX:txtPg7Sc9I1CA", MX_SCHEDULE_9_ROW_1),
    ("frm1702MX:txtPg7Sc9I1CB", MX_SCHEDULE_9_ROW_1),
    ("frm1702MX:txtPg7Sc9I1CF", MX_SCHEDULE_9_ROW_1),
    (
        "frm1702RT:txtPg2Pt4I42MinimumCorporate",
        "Item 42 MCIT Due: an entry (see USER_CHOSEN) that \
         form_1702rt_official.rs recompute clears while the return is not \
         subject to MCIT, which the taxable year decides",
    ),
    (
        "frm1702RT:Pg2Pt4I40IncomeTaxRate",
        "Item 40 Income Tax Rate: the filer's entry from 2020; for a taxable \
         year before 2020 form_1702rt_official.rs recompute sets it to 30% \
         (decision P5-2, calculations.json 1702rt-item41-rate-tax), which \
         the taxable year decides and the page locks",
    ),
    (
        "frm1702RT:txtPg2Pt5I57SpecialAllowable",
        "Part V Item 57: the filer's entry from 2020; for a taxable year \
         before 2020 form_1702rt_official.rs recompute sets it to item 35 \
         times 30% (decision P5-2, calculations.json \
         1702rt-item57-special-tax), which the taxable year decides and the \
         page locks",
    ),
    ("frm1702RT:txtPg2Pt4I54C2", RT_ADD_MORE_LINE),
    ("frm1702RT:txtPg3Sc1I17iC2", RT_ADD_MORE_LINE),
    ("frm1702RT:txtPg3Sc2I4C3", RT_ADD_MORE_LINE),
    ("frm1702RT:txtPg4Sc5I3C2", RT_ADD_MORE_LINE),
    ("frm1702RT:txtPg4Sc5I6C2", RT_ADD_MORE_LINE),
    ("frm1702RT:txtPg4Sc5I8C2", RT_ADD_MORE_LINE),
    (
        "frm1702RT:txtPg4Sc4I1C7",
        "Schedule IV item 1 F) Excess MCIT Applied this Current Taxable \
         Year: typed by the filer; cleared with its row while the return is \
         not subject to MCIT",
    ),
    (
        "frm1701MS:txtShareOfOtherNo20b",
        "Item 20B Less: Share of Other Government Agency (PR 7): typed by the \
         filer as item 20A is (bound to /taxpayer/share_of_other_agencies); \
         form_1701ms.rs `compute` clears it while `share_open` does not hold \
         (item 19B, the spouse's income tax due, is not above zero), which \
         the probe saw move with the spouse's sales",
    ),
];

/// Why each 1702Q schedule entry is in [`GATED_ENTRY`].
const GATED_1702Q_SCHEDULE: &str = "1702Q schedule entry the filer types once its schedule is open \
     (rules/forms/1702q-v2018c/fields.json enabled_when: 'Enabled only by the official \
     dependency/state transition recorded in validations.json or workflow.json.'); \
     form_1702q.rs recompute clears Schedules 1, 2 and 4 while Item 5 has no ATC \
     (tracked rule record rules/forms/1702q-v2018c/validations.json 1702q-change-007 of \
     the untracked official HTA runtime) and Schedule 3 while `mcit_open` does not \
     hold, and overwrites Schedule 2 item 6 under OSD (port behaviour)";

const MX_SCHEDULE_9_ROW_1: &str = "1702MX Schedule 9 item 1 (Year, A, B and F): typed by \
     the filer like items 2 and 3; form_1702mx_official.rs recompute clears \
     every Schedule 9 row while the return is not subject to MCIT \
     (enableMCITFields), which the incorporation date decides";

const RT_ADD_MORE_LINE: &str = "1702RT: the amount of an item with an \"(Add more...)\" \
     table (Part IV item 54, Schedule I item 17i, Schedule II item 4, Schedule V \
     items 3, 6 and 8), typed by the filer like its description; once the table \
     holds two or more rows, form_1702rt_official.rs recompute writes OTHERS and \
     the table's subtotal there (FORM_1702RT_MODALS, saveModalTable)";

/// Project `fields.json` onto the boxes the assistant needs.
pub fn catalog(fields_json: &str) -> Result<Vec<BoxDef>, String> {
    let doc: Value = serde_json::from_str(fields_json).map_err(|error| error.to_string())?;
    let fields = doc["fields"]
        .as_array()
        .ok_or("fields.json has no fields array")?;
    let text = |field: &Value, key: &str| field[key].as_str().unwrap_or_default().to_string();
    Ok(fields
        .iter()
        .map(|field| {
            let values = field["enum_values"].as_array().cloned().unwrap_or_default();
            // A radio's own meaning: `[[{stored_value: "true", choice_meaning, control_value}]]`.
            let chosen = values
                .iter()
                .filter_map(Value::as_array)
                .flatten()
                .find(|meaning| meaning["stored_value"] == "true");
            let key = text(field, "field_key");
            let chosen_by_user = USER_CHOSEN.iter().any(|(k, _)| *k == key);
            let computed_by_form = COMPUTED_BY_FORM.iter().any(|(k, _)| *k == key);
            let typed_by_user = FILER_TYPED.iter().any(|(k, _)| *k == key);
            let from_profile = PROFILE_NOT_COMPUTED.iter().any(|(k, _)| *k == key);
            BoxDef {
                label: text(field, "label"),
                item: text(field, "item_number"),
                kind: text(field, "control_kind"),
                computed: computed_by_form
                    || (!chosen_by_user
                        && !typed_by_user
                        && !from_profile
                        && field["computed"].as_bool().unwrap_or(false)),
                required: if chosen_by_user {
                    "conditional".to_string()
                } else if computed_by_form {
                    "computed".to_string()
                } else if typed_by_user {
                    "optional".to_string()
                } else {
                    text(field, "required")
                },
                key,
                options: values
                    .iter()
                    .filter_map(|value| match value {
                        Value::String(value) => Some(BoxOption {
                            value: value.clone(),
                            label: String::new(),
                        }),
                        Value::Object(_) => Some(BoxOption {
                            value: value["value"].as_str()?.to_string(),
                            label: value["label"].as_str().unwrap_or_default().to_string(),
                        }),
                        _ => None,
                    })
                    .collect(),
                layout_only: false,
                group: None,
                radio_group: false,
                members: Vec::new(),
                choice: chosen
                    .map(|c| text(c, "choice_meaning"))
                    .unwrap_or_default(),
                choice_value: chosen.map(|c| text(c, "control_value")).unwrap_or_default(),
                family: None,
            }
        })
        .collect())
}

/// Draft fields that are bookkeeping, never boxes: the submission
/// lifecycle (flattened into every draft), the row id, and the 2550Q
/// editor's raw capture.
fn skipped(top: &str) -> bool {
    static KEYS: std::sync::OnceLock<BTreeSet<String>> = std::sync::OnceLock::new();
    KEYS.get_or_init(|| {
        let lifecycle = serde_json::to_value(super::super::queueable::SubmissionLifecycle {
            submitted_at: Some(String::new()),
            confirmed_at: Some(String::new()),
            submission_filename: Some(String::new()),
            receipt_id: Some(0),
            submitted_payload: Some(String::new()),
            queued_submission_fingerprint: Some(String::new()),
            submission_claim_token: Some(String::new()),
            submission_claimed_at: Some(String::new()),
            submission_claim_lease_until: Some(String::new()),
            submission_put_started_at: Some(String::new()),
            submission_error: Some(String::new()),
            next_retry_at: Some(String::new()),
            trigger_date_entered: true,
            trigger_parts_typed: vec![String::new()],
            trigger_date_model: true,
            ..Default::default()
        })
        .expect("lifecycle serializes");
        lifecycle
            .as_object()
            .map(|object| object.keys().cloned().collect::<BTreeSet<_>>())
            .unwrap_or_default()
            .into_iter()
            .chain(
                ["id", "queue_authorization", "raw_editor_state"]
                    .into_iter()
                    .map(String::from),
            )
            .collect()
    })
    .contains(top)
}

const TEXT_PROBES: [&str; 2] = ["QZ7X", "7193"];
const AMOUNT_PROBE: f64 = 4321.17;
const WHOLE_PROBES: [i64; 3] = [7, 3, 97];
/// Rows a probed list is grown to, so boxes of later rows are reached too
/// (`extend_rows` carries the pattern to the rest).
const PROBE_ROWS: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ValueKind {
    Text(Option<DateFormatKey>),
    /// One part of a probed date, shown on its own (see [`DatePart`]).
    DatePart(DateFormatKey, DatePart),
    Amount,
    Whole,
}

/// [`DateFormat`] with an order, for sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum DateFormatKey {
    Iso,
    Mdy,
    MonthYear,
}

impl DateFormatKey {
    fn of(text: &str) -> Option<Self> {
        let bytes = text.as_bytes();
        let digits = |range: std::ops::Range<usize>| bytes[range].iter().all(u8::is_ascii_digit);
        match bytes.len() {
            10 if bytes[4] == b'-' && bytes[7] == b'-' && digits(0..4) => Some(Self::Iso),
            10 if bytes[2] == b'/' && bytes[5] == b'/' && digits(6..10) => Some(Self::Mdy),
            7 if bytes[2] == b'/' && digits(3..7) => Some(Self::MonthYear),
            _ => None,
        }
    }

    fn probe(self) -> &'static str {
        match self {
            Self::Iso => "2031-07-19",
            Self::Mdy => "07/19/2031",
            Self::MonthYear => "07/2031",
        }
    }

    fn echoed_by(self, official: &str) -> bool {
        let digits: String = official.chars().filter(char::is_ascii_digit).collect();
        match self {
            Self::Iso | Self::Mdy => digits == "07192031" || digits == "20310719",
            Self::MonthYear => digits == "072031" || digits == "203107",
        }
    }

    /// The part of the probe date a box shows on its own, if it shows
    /// exactly one: `07` (month), `19` (day) or `2031` (year).
    fn part_shown_by(self, official: &str) -> Option<DatePart> {
        let digits: String = official.chars().filter(char::is_ascii_digit).collect();
        if digits.len() != official.trim().len() {
            return None;
        }
        match digits.as_str() {
            "07" | "7" => Some(DatePart::Month),
            "19" if self != Self::MonthYear => Some(DatePart::Day),
            "2031" => Some(DatePart::Year),
            _ => None,
        }
    }

    fn format(self) -> DateFormat {
        match self {
            Self::Iso => DateFormat::Iso,
            Self::Mdy => DateFormat::Mdy,
            Self::MonthYear => DateFormat::MonthYear,
        }
    }
}

enum Candidate {
    /// Echo probe: (kind, nullable, value).
    Value(ValueKind, bool, Value),
    /// A flag or enum value.
    Choice(Value),
}

fn echoes(kind: ValueKind, probe: &Value, official: &str) -> bool {
    match kind {
        ValueKind::Text(Some(date)) => date.echoed_by(official),
        ValueKind::Text(None) => probe
            .as_str()
            .is_some_and(|probe| official.to_ascii_uppercase().contains(probe)),
        ValueKind::Amount => parse_amount(official).is_some_and(|value| {
            (value - AMOUNT_PROBE).abs() < 0.005
                || value == AMOUNT_PROBE.round()
                || (value - (AMOUNT_PROBE * 10.0).round() / 10.0).abs() < 0.005
        }),
        ValueKind::Whole => official.trim().parse::<i64>().ok() == probe.as_i64(),
        ValueKind::DatePart(date, part) => date.part_shown_by(official) == Some(part),
    }
}

/// Every leaf of a draft: JSON pointer -> value.
fn leaves(value: &Value, path: &str, depth: usize, out: &mut Vec<(String, Value)>) {
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                if depth == 0 && skipped(key) {
                    continue;
                }
                leaves(child, &format!("{path}/{key}"), depth + 1, out);
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                leaves(child, &format!("{path}/{index}"), depth + 1, out);
            }
        }
        leaf => out.push((path.to_string(), leaf.clone())),
    }
}

fn pointer_set(root: &mut Value, path: &str, leaf: Value) {
    if let Some(slot) = root.pointer_mut(path) {
        *slot = leaf;
    }
}

/// The variants serde lists for an unknown enum value.
fn variants(error: &str) -> Vec<String> {
    let Some(at) = error.find("expected") else {
        return Vec::new();
    };
    error[at..]
        .split('`')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect()
}

fn parses(kind: AgentFormKind, base: &Value, path: &str, leaf: &Value) -> Result<(), String> {
    let mut draft = base.clone();
    pointer_set(&mut draft, path, leaf.clone());
    match parse_error(kind, &draft) {
        None => Ok(()),
        Some(error) => Err(error),
    }
}

fn candidates(kind: AgentFormKind, base: &Value, path: &str, leaf: &Value) -> Vec<Candidate> {
    let fits = |probe: &Value| parses(kind, base, path, probe).is_ok();
    let enum_values = |current: &Value| -> Vec<Candidate> {
        match parses(kind, base, path, &Value::from(TEXT_PROBES[0])) {
            Err(error) if error.contains("unknown variant") => variants(&error)
                .into_iter()
                .map(Value::String)
                .filter(|variant| variant != current && fits(variant))
                .map(Candidate::Choice)
                .collect(),
            _ => Vec::new(),
        }
    };
    match leaf {
        Value::Bool(flag) => vec![Candidate::Choice(Value::Bool(!flag))],
        Value::Number(number) if number.is_f64() => {
            let probe = Value::from(AMOUNT_PROBE);
            if fits(&probe) {
                vec![Candidate::Value(ValueKind::Amount, false, probe)]
            } else {
                Vec::new()
            }
        }
        Value::Number(number) => WHOLE_PROBES
            .iter()
            .map(|probe| Value::from(*probe))
            .find(|probe| number.as_i64() != probe.as_i64() && fits(probe))
            .map(|probe| vec![Candidate::Value(ValueKind::Whole, false, probe)])
            .unwrap_or_default(),
        Value::String(text) => {
            if let Some(date) = DateFormatKey::of(text) {
                let probe = Value::from(date.probe());
                if fits(&probe) {
                    return vec![Candidate::Value(ValueKind::Text(Some(date)), false, probe)];
                }
            }
            let mut out: Vec<Candidate> = TEXT_PROBES
                .iter()
                .map(|probe| Value::from(*probe))
                .filter(|probe| fits(probe))
                .map(|probe| Candidate::Value(ValueKind::Text(None), false, probe))
                .collect();
            if out.is_empty() {
                out = enum_values(leaf);
            }
            out
        }
        Value::Null => {
            let amount = Value::from(AMOUNT_PROBE);
            if fits(&amount) {
                return vec![Candidate::Value(ValueKind::Amount, true, amount)];
            }
            let whole = Value::from(WHOLE_PROBES[0]);
            if fits(&whole) {
                return vec![Candidate::Value(ValueKind::Whole, true, whole)];
            }
            for date in [
                DateFormatKey::Iso,
                DateFormatKey::Mdy,
                DateFormatKey::MonthYear,
            ] {
                let probe = Value::from(date.probe());
                if parses(kind, base, path, &Value::from(TEXT_PROBES[0])).is_err() && fits(&probe) {
                    return vec![Candidate::Value(ValueKind::Text(Some(date)), true, probe)];
                }
            }
            let texts: Vec<Candidate> = TEXT_PROBES
                .iter()
                .map(|probe| Value::from(*probe))
                .filter(|probe| fits(probe))
                .map(|probe| Candidate::Value(ValueKind::Text(None), true, probe))
                .collect();
            if !texts.is_empty() {
                return texts;
            }
            if fits(&Value::Bool(true)) {
                return vec![Candidate::Choice(Value::Bool(true))];
            }
            enum_values(leaf)
        }
        _ => Vec::new(),
    }
}

/// One way a choice box took an official value.
#[derive(Debug, Clone)]
struct ChoiceObs {
    seed: usize,
    /// The probed leaf, or `None` for the seed itself.
    change: Option<(String, Value)>,
    official: String,
    answered: bool,
}

#[derive(Default)]
struct Evidence {
    /// key -> (path, kind, nullable)
    echoes: BTreeMap<String, BTreeSet<(String, ValueKind, bool)>>,
    choices: BTreeMap<String, Vec<ChoiceObs>>,
    /// key -> paths whose probe changed it
    touched: BTreeMap<String, BTreeSet<String>>,
    /// path -> how many keys its probe echoed
    echo_count: BTreeMap<String, usize>,
    /// (key, path) echoes seen on a real sample, not only on one whose
    /// lists were grown (a grown list can spill into other boxes).
    real_echoes: BTreeSet<(String, String)>,
    /// (key, path) where a probe of the path changed the box without
    /// showing the probe: the box is a sum or a copy with a twist.
    misses: BTreeSet<(String, String)>,
    identity_paths: BTreeSet<String>,
    /// keys any probe or seed wrote
    present: BTreeSet<String>,
    /// Per seed: path -> base value.
    seed_leaves: BTreeMap<usize, BTreeMap<String, Value>>,
    /// path -> base string values seen
    strings: BTreeMap<String, BTreeSet<String>>,
    /// Flag or option path -> keys its choice probes changed.
    gates: BTreeMap<String, BTreeSet<String>>,
}

/// Grow every non-empty list to [`PROBE_ROWS`] by repeating its last row.
fn grow_rows(value: &mut Value) {
    match value {
        Value::Array(items) => {
            for item in items.iter_mut() {
                grow_rows(item);
            }
            if let Some(last) = items.last().cloned() {
                while items.len() < PROBE_ROWS {
                    items.push(last.clone());
                }
            }
        }
        Value::Object(object) => {
            for (key, child) in object.iter_mut() {
                if !skipped(key) {
                    grow_rows(child);
                }
            }
        }
        _ => {}
    }
}

fn blank(value: &Value) -> Value {
    match value {
        Value::String(_) => Value::String(String::new()),
        Value::Number(number) if number.is_f64() => Value::from(0.0),
        Value::Number(_) => Value::from(0),
        Value::Bool(_) => Value::Bool(false),
        Value::Array(_) => Value::Array(Vec::new()),
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, child)| (key.clone(), blank(child)))
                .collect(),
        ),
        Value::Null => Value::Null,
    }
}

/// A blank row for every list of `seed`, keyed by its pattern: each leaf
/// of the last row blanked where the draft still parses with it (an enum
/// keeps the sample's variant).
fn row_templates(kind: AgentFormKind, seed: &Value, out: &mut BTreeMap<String, Value>) {
    let mut lists = Vec::new();
    collect_lists(seed, "", 0, &mut lists);
    for (path, pattern) in lists {
        if out.contains_key(&pattern) {
            continue;
        }
        let Some(Value::Array(items)) = seed.pointer(&path) else {
            continue;
        };
        let Some(last) = items.last() else {
            continue;
        };
        let with_row = |row: &Value| {
            let mut draft = seed.clone();
            if let Some(Value::Array(items)) = draft.pointer_mut(&path) {
                items.push(row.clone());
            }
            parse_error(kind, &draft).is_none()
        };
        let candidate = blank(last);
        let template = if with_row(&candidate) {
            candidate
        } else {
            let mut row = last.clone();
            let mut row_leaves = Vec::new();
            leaves(last, "", 1, &mut row_leaves);
            for (leaf_path, leaf) in row_leaves {
                // A blank, else null (an optional identity such as 2550Q's
                // row instance_id, which an empty string does not parse as):
                // a copied value would give every added row the same one.
                for candidate in [blank(&leaf), Value::Null] {
                    let mut trial = row.clone();
                    pointer_set(&mut trial, &leaf_path, candidate);
                    if with_row(&trial) {
                        row = trial;
                        break;
                    }
                }
            }
            row
        };
        out.insert(pattern, template);
    }
}

fn collect_lists(value: &Value, path: &str, depth: usize, out: &mut Vec<(String, String)>) {
    match value {
        Value::Array(items) => {
            if !items.is_empty() {
                let segments: Vec<&str> = path.split('/').skip(1).collect();
                out.push((path.to_string(), super::row_pattern(&segments)));
            }
            for (index, item) in items.iter().enumerate() {
                collect_lists(item, &format!("{path}/{index}"), depth + 1, out);
            }
        }
        Value::Object(object) => {
            for (key, child) in object {
                if depth == 0 && skipped(key) {
                    continue;
                }
                collect_lists(child, &format!("{path}/{key}"), depth + 1, out);
            }
        }
        _ => {}
    }
}

fn radio_answered(
    boxes: &BTreeMap<String, &BoxDef>,
    def: &AgentFormDef,
    key: &str,
    map: &BTreeMap<String, String>,
) -> bool {
    if boxes.get(key).is_none_or(|entry| !entry.is_radio()) {
        return true;
    }
    family_radios(def, key)
        .iter()
        .filter(|radio| map.get(&radio.key).is_some_and(|value| value == "true"))
        .count()
        == 1
}

/// `frm1601c:AmendedRtn_1` -> `frm1601c:AmendedRtn`: a first guess at the
/// radios of one item while probing (the pinned groups come from the
/// bindings, see [`group_radios`]).
fn radio_family(key: &str) -> &str {
    match key.rfind('_') {
        Some(at) if at > 0 && key.len() - at <= 4 => &key[..at],
        _ => key.trim_end_matches(|c: char| c.is_ascii_digit()),
    }
}

fn family_radios<'a>(def: &'a AgentFormDef, key: &str) -> Vec<&'a BoxDef> {
    let Some(entry) = def.box_def(key).filter(|entry| entry.is_radio()) else {
        return Vec::new();
    };
    let family = radio_family(&entry.key);
    def.boxes
        .iter()
        .filter(|other| {
            other.is_radio() && other.item == entry.item && radio_family(&other.key) == family
        })
        .collect()
}

/// Suffixes and option values a code-like string field may take, read off
/// the choice boxes its probe changed: `CatAgent_P` -> `P`, a select's
/// option values.
fn code_candidates(shell: &AgentFormDef, keys: &[String]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for key in keys {
        let Some(entry) = shell.box_def(key) else {
            continue;
        };
        if entry.is_radio() || entry.kind.contains("checkbox") {
            for radio in family_radios(shell, key).into_iter().chain([entry]) {
                if let Some((_, suffix)) = radio.key.rsplit_once(['_', ':'])
                    && (1..=3).contains(&suffix.len())
                {
                    out.insert(suffix.to_string());
                }
            }
        }
        if entry.kind.contains("select") {
            out.extend(
                entry
                    .options
                    .iter()
                    .map(|option| option.value.clone())
                    .filter(|value| !value.is_empty()),
            );
        }
    }
    out
}

fn is_choice_box(entry: &BoxDef) -> bool {
    ["radio", "checkbox", "select"]
        .iter()
        .any(|kind| entry.kind.contains(kind))
}

fn probe_seed(
    kind: AgentFormKind,
    seed_index: usize,
    (seed, grown): (&Value, bool),
    shell: &AgentFormDef,
    evidence: &mut Evidence,
) {
    let Some((base, base_map)) = probe_eval(kind, seed) else {
        return;
    };
    let by_key: BTreeMap<String, &BoxDef> = shell
        .boxes
        .iter()
        .map(|entry| (entry.key.clone(), entry))
        .collect();
    let base_identity = probe_identity(kind, &base);
    let mut base_leaves = Vec::new();
    leaves(&base, "", 0, &mut base_leaves);
    evidence.present.extend(base_map.keys().cloned());
    for (key, official) in &base_map {
        evidence
            .choices
            .entry(key.clone())
            .or_default()
            .push(ChoiceObs {
                seed: seed_index,
                change: None,
                official: official.clone(),
                answered: radio_answered(&by_key, shell, key, &base_map),
            });
    }
    for (path, leaf) in &base_leaves {
        if let Value::String(text) = leaf {
            evidence
                .strings
                .entry(path.clone())
                .or_default()
                .insert(text.clone());
        }
    }
    evidence
        .seed_leaves
        .insert(seed_index, base_leaves.iter().cloned().collect());
    // Probe one candidate; returns the keys it changed without echoing.
    let run = |path: &String, candidate: &Candidate, evidence: &mut Evidence| -> Vec<String> {
        let probe_value = match candidate {
            Candidate::Value(_, _, value) | Candidate::Choice(value) => value.clone(),
        };
        let mut draft = base.clone();
        pointer_set(&mut draft, path, probe_value);
        let Some((probed, map)) = probe_eval(kind, &draft) else {
            return Vec::new();
        };
        if probe_identity(kind, &probed) != base_identity {
            evidence.identity_paths.insert(path.clone());
        }
        evidence.present.extend(map.keys().cloned());
        let changed: BTreeSet<&String> = map
            .keys()
            .chain(base_map.keys())
            .filter(|key| map.get(*key) != base_map.get(*key))
            .collect();
        let mut silent = Vec::new();
        for key in changed {
            evidence
                .touched
                .entry(key.clone())
                .or_default()
                .insert(path.clone());
            let official = map.get(key).cloned().unwrap_or_default();
            match candidate {
                Candidate::Value(value_kind, nullable, probe) => {
                    // A box that shows one part of the probed date (a month
                    // select, a day box) echoes that part.
                    let value_kind = match value_kind {
                        ValueKind::Text(Some(date)) if !echoes(*value_kind, probe, &official) => {
                            date.part_shown_by(&official)
                                .map(|part| ValueKind::DatePart(*date, part))
                                .unwrap_or(*value_kind)
                        }
                        _ => *value_kind,
                    };
                    let value_kind = &value_kind;
                    if echoes(*value_kind, probe, &official) {
                        evidence.echoes.entry(key.clone()).or_default().insert((
                            path.clone(),
                            *value_kind,
                            *nullable,
                        ));
                        *evidence.echo_count.entry(path.clone()).or_default() += 1;
                        if !grown {
                            evidence.real_echoes.insert((key.clone(), path.clone()));
                        }
                    } else {
                        evidence.misses.insert((key.clone(), path.clone()));
                        silent.push(key.clone());
                    }
                }
                Candidate::Choice(value) => {
                    evidence
                        .gates
                        .entry(path.clone())
                        .or_default()
                        .insert(key.clone());
                    evidence
                        .choices
                        .entry(key.clone())
                        .or_default()
                        .push(ChoiceObs {
                            seed: seed_index,
                            change: Some((path.clone(), value.clone())),
                            official,
                            answered: radio_answered(&by_key, shell, key, &map),
                        });
                }
            }
        }
        silent
    };
    for (path, leaf) in &base_leaves {
        let mut silent = Vec::new();
        for candidate in candidates(kind, &base, path, leaf) {
            silent.extend(run(path, &candidate, evidence));
        }
        // A code kept in a plain string (`"P"`/`"G"`, an option code):
        // try the codes the choice boxes it moved can show.
        if leaf.is_string() {
            let choice_keys: Vec<String> = silent
                .into_iter()
                .filter(|key| by_key.get(key).is_some_and(|entry| is_choice_box(entry)))
                .collect();
            for code in code_candidates(shell, &choice_keys)
                .into_iter()
                .take(MAX_CODES)
            {
                if leaf.as_str() == Some(code.as_str())
                    || parses(kind, &base, path, &Value::from(code.as_str())).is_err()
                {
                    continue;
                }
                run(path, &Candidate::Choice(Value::from(code)), evidence);
            }
        }
    }
}

fn is_election(path: &str) -> bool {
    path.to_ascii_lowercase().contains("election")
}

/// Gates opened per form in the second pass, and variants per gate.
const MAX_GATES: usize = 12;
const MAX_GATE_VALUES: usize = 4;
/// Codes tried per string field (a long select such as the RDO list is
/// sampled, not walked).
const MAX_CODES: usize = 24;

/// Derive the bindings of `kind`'s boxes from sample drafts.
///
/// `profile_values` are the sample taxpayer profile's strings (name,
/// address, ...): a string field holding one of them is the profile's.
///
/// A first pass probes every leaf of each sample (and of the sample with
/// its lists grown). Boxes still unbound are often shown only behind a flag
/// (Amended Return, Tax Relief): a second pass flips each flag or option
/// that moved one of them and probes again.
pub fn derive(
    kind: AgentFormKind,
    seeds: &[Value],
    profile_values: &[String],
    boxes: Vec<BoxDef>,
) -> AgentFormDef {
    let mut def = AgentFormDef {
        package: kind.package().to_string(),
        form_code: kind.form_code().to_string(),
        boxes,
        bindings: BTreeMap::new(),
        rows: BTreeMap::new(),
        unmapped: BTreeMap::new(),
    };
    let mut evidence = Evidence::default();
    let mut probed_seeds = Vec::new();
    for seed in seeds {
        if let Some((computed, _)) = probe_eval(kind, seed) {
            row_templates(kind, &computed, &mut def.rows);
            let mut grown = computed.clone();
            grow_rows(&mut grown);
            let was_grown = grown != computed;
            probed_seeds.push((computed, false));
            if was_grown {
                probed_seeds.push((grown, true));
            }
        }
    }
    for (index, (seed, grown)) in probed_seeds.iter().enumerate() {
        probe_seed(kind, index, (seed, *grown), &def, &mut evidence);
    }
    let profile: BTreeSet<String> = profile_values
        .iter()
        .filter(|value| !value.trim().is_empty())
        .cloned()
        .collect();
    let user_keys: Vec<String> = def
        .boxes
        .iter()
        .filter(|entry| entry.is_user_box())
        .map(|entry| entry.key.clone())
        .collect();

    // Second pass: open the gates of the boxes still unbound (or bound only
    // through a grown list), then the busiest other flags.
    let unresolved: BTreeSet<&String> = user_keys
        .iter()
        .filter(|key| match decide(&def, &evidence, &profile, key) {
            Err(_) => true,
            Ok(_) => evidence.echoes.get(*key).is_some_and(|echoes| {
                echoes.iter().all(|(path, _, _)| {
                    !evidence
                        .real_echoes
                        .contains(&((*key).clone(), path.clone()))
                })
            }),
        })
        .collect();
    let mut gates: Vec<((usize, usize), &String)> = evidence
        .gates
        .iter()
        .map(|(path, keys)| {
            let open = keys.iter().filter(|key| unresolved.contains(key)).count();
            ((open, keys.len()), path)
        })
        .collect();
    gates.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
    let mut variants = Vec::new();
    for ((open, _), path) in gates.into_iter().take(MAX_GATES) {
        // A flag that moved a box still unbound is opened on every real
        // sample; a merely busy one on the first sample that has it.
        let reals = probed_seeds
            .iter()
            .filter(|(seed, grown)| !grown && seed.pointer(path).is_some())
            .take(if open > 0 { usize::MAX } else { 1 });
        for (seed, _) in reals {
            let leaf = seed.pointer(path).expect("filtered above");
            for candidate in candidates(kind, seed, path, leaf)
                .into_iter()
                .take(if open > 0 { MAX_GATE_VALUES } else { 1 })
            {
                if let Candidate::Choice(value) = candidate {
                    let mut variant = seed.clone();
                    pointer_set(&mut variant, path, value);
                    variants.push((variant, false));
                }
            }
        }
    }
    let first = probed_seeds.len();
    for (offset, (variant, grown)) in variants.iter().enumerate() {
        probe_seed(kind, first + offset, (variant, *grown), &def, &mut evidence);
    }

    for key in &user_keys {
        match decide(&def, &evidence, &profile, key) {
            Ok(bound) => {
                def.bindings.insert(key.clone(), bound);
            }
            Err(reason) => {
                def.unmapped.insert(key.clone(), reason);
            }
        }
    }
    extend_rows(&mut def);
    let real: Vec<&Value> = probed_seeds
        .iter()
        .filter(|(_, grown)| !grown)
        .map(|(seed, _)| seed)
        .collect();
    verify(kind, &mut def, &real);
    group_radios(&mut def);
    def
}

/// `(item number, draft fields its radios set)`.
type RadioItem = (String, Vec<String>);

/// Radio keys with the draft fields each sets when checked.
type Radios = Vec<(String, BTreeMap<String, Value>)>;

/// One box per radio item. Radios (and checkboxes acting as radios) whose
/// bindings set the same draft fields, each to a different value when
/// checked, answer one question: they get a group box keyed by their common
/// key prefix (`frm0619E:optWithheld:Y` + `:N` -> `frm0619E:optWithheld`),
/// labelled by their common caption, with one option per radio (its key
/// suffix as the value, the rest of its caption as the label). The group is
/// filled with an option value; each radio stays fillable by its own key.
pub fn group_radios(def: &mut AgentFormDef) {
    let mut items: BTreeMap<RadioItem, Radios> = BTreeMap::new();
    for entry in def.boxes.iter().filter(|entry| {
        entry.is_user_box() && (entry.is_radio() || entry.kind.contains("checkbox"))
    }) {
        let Some(BoundBox {
            binding: Binding::Choice { options },
            ..
        }) = def.bindings.get(&entry.key)
        else {
            continue;
        };
        let Some(checked) = options.iter().find(|option| option.value == "true") else {
            continue;
        };
        let fields: Vec<String> = options
            .iter()
            .flat_map(|option| option.set.keys().cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let item = RADIO_ITEM
            .iter()
            .find(|(form, key, ..)| *form == def.form_code && *key == entry.key)
            .map_or_else(|| entry.item.clone(), |(_, _, item, _)| (*item).to_string());
        items
            .entry((item, fields))
            .or_default()
            .push((entry.key.clone(), checked.set.clone()));
    }
    // Radios of one item whose keys share a stem (`optATC_1`..`optATC_6`) are
    // one question even when they set different fields (8% vs graduated).
    let mut merged: BTreeMap<(String, String), Radios> = BTreeMap::new();
    for ((item, _), radios) in items {
        let keys: Vec<&str> = radios.iter().map(|(key, _)| key.as_str()).collect();
        let stem = match keys.as_slice() {
            // A lone radio: its key without the option suffix (`..._2`).
            [key] => option_stem(key),
            _ => common_prefix(&keys),
        }
        .trim_end_matches([':', '_', '-', '.'])
        .to_string();
        merged.entry((item, stem)).or_default().extend(radios);
    }
    for ((item, _), radios) in merged {
        let distinct: BTreeSet<String> = radios
            .iter()
            .map(|(_, set)| serde_json::to_string(set).unwrap_or_default())
            .collect();
        if radios.len() < 2 || distinct.len() != radios.len() {
            continue;
        }
        let keys: Vec<&str> = radios.iter().map(|(key, _)| key.as_str()).collect();
        let prefix = common_prefix(&keys);
        let mut group = prefix.trim_end_matches([':', '_', '-', '.']).to_string();
        // Keys that share only the form prefix (`frm2550qv2024:calendarNo1`
        // and `:fiscalNo1`) are named by their item.
        let radios_def: Vec<&BoxDef> = keys.iter().filter_map(|key| def.box_def(key)).collect();
        let labels: Vec<String> = radios_def.iter().map(|entry| entry.label.clone()).collect();
        let number = if item.is_empty() {
            labels[0]
                .split_whitespace()
                .next()
                .filter(|word| word.chars().next().is_some_and(|c| c.is_ascii_digit()))
                .unwrap_or("")
                .to_string()
        } else {
            item.clone()
        };
        if keys.iter().all(|key| key.contains(':')) && !group.contains(':') && !number.is_empty() {
            group = format!("{group}:item{number}");
        }
        if group.is_empty() || def.box_def(&group).is_some() {
            group = format!("{prefix}group");
        }
        let distinct_labels = labels.iter().collect::<BTreeSet<_>>().len() == labels.len();
        let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        let shared = if distinct_labels {
            word_prefix(&common_prefix(&label_refs))
        } else {
            String::new()
        };
        // A yes/no item: one flag, set true by one radio and false by the other.
        let yes_no: Option<Vec<&str>> = {
            let flags: Vec<Option<bool>> = radios
                .iter()
                .map(
                    |(_, set)| match set.values().collect::<Vec<_>>().as_slice() {
                        [Value::Bool(flag)] if set.len() == 1 => Some(*flag),
                        _ => None,
                    },
                )
                .collect();
            (flags.len() == 2 && flags[0].is_some() && flags[1].is_some() && flags[0] != flags[1])
                .then(|| {
                    flags
                        .iter()
                        .map(|flag| if *flag == Some(true) { "Yes" } else { "No" })
                        .collect()
                })
        };
        let codes: Vec<String> = radios_def
            .iter()
            .map(|entry| entry.choice_value.clone())
            .collect();
        let use_codes = codes.iter().all(|code| !code.is_empty())
            && codes.iter().collect::<BTreeSet<_>>().len() == codes.len();
        let options: Vec<BoxOption> = keys
            .iter()
            .zip(&radios_def)
            .enumerate()
            .map(|(at, (key, entry))| {
                let value = if use_codes {
                    codes[at].clone()
                } else {
                    key[prefix.len()..]
                        .trim_start_matches([':', '_', '-', '.'])
                        .to_string()
                };
                let rest = entry.label[shared.len().min(entry.label.len())..]
                    .trim()
                    .to_string();
                let label = if !entry.choice.is_empty() {
                    entry.choice.clone()
                } else if distinct_labels && !rest.is_empty() {
                    rest
                } else if let Some(yes_no) = &yes_no {
                    yes_no[at].to_string()
                } else {
                    value.clone()
                };
                BoxOption { label, value }
            })
            .collect();
        if options.iter().any(|option| option.value.is_empty())
            || options
                .iter()
                .map(|option| &option.value)
                .collect::<BTreeSet<_>>()
                .len()
                != options.len()
        {
            continue;
        }
        let group_label = if !distinct_labels {
            labels[0].clone()
        } else if shared.trim().chars().all(|c| c.is_ascii_alphanumeric())
            && !shared.trim().is_empty()
            && shared
                .trim()
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_digit())
        {
            // Only the item number is shared (`13 Micro`, `13 Small`).
            format!("Item {}", shared.trim())
        } else {
            let trimmed = shared.trim().trim_end_matches([':', '-', '?', ' ']).trim();
            if trimmed.is_empty() {
                labels[0].clone()
            } else if shared.trim().ends_with('?') {
                format!("{trimmed}?")
            } else {
                trimmed.to_string()
            }
        };
        let members = &radios_def;
        let required = if members.iter().any(|entry| entry.required == "required") {
            "required"
        } else if members.iter().any(|entry| entry.required == "conditional") {
            "conditional"
        } else {
            "optional"
        };
        let lock = keys
            .iter()
            .find_map(|key| def.bindings.get(*key).and_then(|bound| bound.lock));
        let group_box = BoxDef {
            key: group.clone(),
            label: group_label,
            item: item.clone(),
            kind: "radio".to_string(),
            computed: false,
            required: required.to_string(),
            options: options.clone(),
            layout_only: false,
            group: None,
            radio_group: true,
            members: keys.iter().map(|key| key.to_string()).collect(),
            choice: String::new(),
            choice_value: String::new(),
            family: None,
        };
        let binding = Binding::Choice {
            options: options
                .iter()
                .zip(&radios)
                .map(|(option, (_, set))| ChoiceOption {
                    value: option.value.clone(),
                    set: set.clone(),
                })
                .collect(),
        };
        for entry in def.boxes.iter_mut() {
            if keys.contains(&entry.key.as_str()) {
                entry.group = Some(group.clone());
            }
        }
        let at = def
            .boxes
            .iter()
            .position(|entry| entry.key == keys[0])
            .unwrap_or(def.boxes.len());
        def.boxes.insert(at, group_box);
        def.bindings
            .insert(group.clone(), BoundBox { binding, lock });
    }
}

/// `frm1701q:optMethodOfDeduction:_2` -> `frm1701q:optMethodOfDeduction:`,
/// `frm1701q:optType_3` -> `frm1701q:optType_`.
fn option_stem(key: &str) -> String {
    match key.rfind([':', '_', '-']) {
        Some(at) if key.len() - at <= 5 => key[..=at].to_string(),
        _ => key
            .trim_end_matches(|c: char| c.is_ascii_digit())
            .to_string(),
    }
}

fn common_prefix(texts: &[&str]) -> String {
    let Some(first) = texts.first() else {
        return String::new();
    };
    let mut end = first.len();
    for text in &texts[1..] {
        end = first
            .char_indices()
            .zip(text.chars())
            .take_while(|((_, a), b)| a == b)
            .last()
            .map_or(0, |((at, c), _)| at + c.len_utf8())
            .min(end);
    }
    first[..end].to_string()
}

/// Cut a shared caption back to a whole word (`"3 Amended Form? Ye"` ->
/// `"3 Amended Form? "`).
fn word_prefix(prefix: &str) -> String {
    match prefix.rfind(|c: char| c.is_whitespace() || matches!(c, ':' | '-' | '(' | '/')) {
        Some(at) => prefix[..=at].to_string(),
        None => String::new(),
    }
}

/// Labels a user can read. `fields.json` names many boxes only by their
/// key; for those take the caption printed beside the box on the official
/// sheet (`captions`: writer key -> printed text, from
/// `html-frozen/<sheet>/writer-cells.json`), else `Item N`, else the words of
/// the key itself (`txtPSIC` -> `PSIC`). Boxes added from
/// the app's field map take the printed caption first.
pub fn apply_labels(boxes: &mut [BoxDef], captions: &BTreeMap<String, String>) {
    // A radio whose label its siblings share ("Amended Return" twice) says
    // more with its printed caption ("3 Amended Return? Yes").
    let mut shared: BTreeMap<(String, String), usize> = BTreeMap::new();
    for entry in boxes.iter().filter(|entry| entry.is_radio()) {
        *shared
            .entry((entry.item.clone(), entry.label.clone()))
            .or_default() += 1;
    }
    for entry in boxes.iter_mut() {
        let raw = entry.label.trim().is_empty() || entry.label == entry.key;
        let ambiguous = entry.is_radio()
            && shared
                .get(&(entry.item.clone(), entry.label.clone()))
                .is_some_and(|count| *count > 1);
        if !raw && !entry.layout_only && !ambiguous {
            continue;
        }
        if let Some(caption) = captions.get(&entry.key) {
            entry.label = caption.clone();
        } else if raw && !entry.item.is_empty() {
            entry.label = format!("Item {}", entry.item);
        } else if raw {
            // Nothing printed and no item: the key's own words.
            let control = entry.key.rsplit(':').next().unwrap_or(&entry.key);
            let label = label_from_key(control);
            if !label.is_empty() {
                entry.label = label;
            }
        }
    }
}

/// Fill every binding into each real sample the way `agent::fill` does and
/// keep it only when the box then shows the value (or, behind an unanswered
/// item, keeps its old one). Rows a form re-sorts or merges fail here.
fn verify(kind: AgentFormKind, def: &mut AgentFormDef, seeds: &[&Value]) {
    let mut rejected = Vec::new();
    for (key, bound) in &def.bindings {
        let trials: Vec<(BTreeMap<String, Value>, String)> = match &bound.binding {
            Binding::Text { path, date, .. } => {
                let probes: Vec<&str> = match date {
                    Some(DateFormat::Iso) => vec!["2031-07-19"],
                    Some(DateFormat::Mdy) => vec!["07/19/2031"],
                    Some(DateFormat::MonthYear) => vec!["07/2031"],
                    None => TEXT_PROBES.to_vec(),
                };
                probes
                    .into_iter()
                    .map(|probe| {
                        (
                            BTreeMap::from([(path.clone(), Value::from(probe))]),
                            probe.to_string(),
                        )
                    })
                    .collect()
            }
            Binding::Amount { path, .. } => vec![(
                BTreeMap::from([(path.clone(), Value::from(AMOUNT_PROBE))]),
                AMOUNT_PROBE.to_string(),
            )],
            Binding::Whole { path, .. } => vec![(
                BTreeMap::from([(path.clone(), Value::from(WHOLE_PROBES[0]))]),
                WHOLE_PROBES[0].to_string(),
            )],
            Binding::Choice { options } => options
                .iter()
                .map(|option| (option.set.clone(), option.value.clone()))
                .collect(),
        };
        let mut shown_once = false;
        let mut wrong = None;
        for seed in seeds {
            let Some((_, base_map)) = probe_eval(kind, seed) else {
                continue;
            };
            if !base_map.contains_key(key) {
                continue;
            }
            for (set, expected) in &trials {
                let mut draft = (*seed).clone();
                if set.iter().any(|(path, leaf)| {
                    super::set_path(&mut draft, path, leaf.clone(), &def.rows).is_err()
                }) {
                    continue;
                }
                let Some((_, map)) = probe_eval(kind, &draft) else {
                    continue;
                };
                let shown = map.get(key).cloned().unwrap_or_default();
                let ok = match &bound.binding {
                    Binding::Text {
                        part: Some(part), ..
                    } => DateFormatKey::Mdy.part_shown_by(&shown) == Some(*part),
                    Binding::Text { date: Some(_), .. } => DateFormatKey::Mdy.echoed_by(&shown),
                    Binding::Text { .. } => shown.to_ascii_uppercase().contains(expected.as_str()),
                    Binding::Amount { .. } => echoes(ValueKind::Amount, &Value::Null, &shown),
                    Binding::Whole { .. } => {
                        shown.trim().parse::<i64>().ok() == Some(WHOLE_PROBES[0])
                    }
                    Binding::Choice { .. } => shown == *expected,
                };
                if ok {
                    shown_once = true;
                } else if map.get(key) != base_map.get(key) {
                    wrong = Some(shown);
                }
            }
        }
        if let Some(shown) = wrong.filter(|_| !shown_once) {
            rejected.push((
                key.clone(),
                format!(
                    "derived by the app: filling it shows `{shown}` (the form re-sorts, merges or recomputes it)"
                ),
            ));
        }
    }
    for (key, reason) in rejected {
        def.bindings.remove(&key);
        def.unmapped.insert(key, reason);
    }
}

/// `frm1601EQ:txtTaxBase7` -> (`frm1601EQ:txtTaxBase`, 7, ``): the last
/// digit run of a key.
fn split_index(key: &str) -> Option<(&str, usize, &str)> {
    let end = key.rfind(|c: char| c.is_ascii_digit())? + 1;
    let start = key[..end]
        .rfind(|c: char| !c.is_ascii_digit())
        .map_or(0, |at| at + 1);
    Some((&key[..start], key[start..end].parse().ok()?, &key[end..]))
}

/// The one row index in a binding's paths, with the pointer split around it.
fn row_of(binding: &Binding) -> Option<(usize, Vec<(String, String)>)> {
    let paths: Vec<&String> = match binding {
        Binding::Text { path, .. } | Binding::Amount { path, .. } | Binding::Whole { path, .. } => {
            vec![path]
        }
        Binding::Choice { .. } => return None,
    };
    let mut row = None;
    let mut parts = Vec::new();
    for path in paths {
        let segments: Vec<&str> = path.split('/').collect();
        let indices: Vec<usize> = segments
            .iter()
            .enumerate()
            .filter(|(_, segment)| segment.parse::<usize>().is_ok())
            .map(|(at, _)| at)
            .collect();
        let [at] = indices.as_slice() else {
            return None;
        };
        let index: usize = segments[*at].parse().ok()?;
        if row.is_some_and(|row| row != index) {
            return None;
        }
        row = Some(index);
        parts.push((segments[..*at].join("/"), segments[at + 1..].join("/")));
    }
    Some((row?, parts))
}

/// `(key prefix, key suffix, number - row, list pointer, field in the row)`.
type RowPattern = (String, String, isize, String, String);

/// Bind the numbered boxes of rows past the probed ones: when at least two
/// probed rows bind `<prefix><n><suffix>` to `<list>/<n - offset>/<field>`
/// with the same offset, every unbound `<prefix><m><suffix>` box of the form
/// is row `m - offset` of that list.
fn extend_rows(def: &mut AgentFormDef) {
    // A row family's row number is where the family says, not the key's
    // last number (`txtPg2Pt4I54.5C1` is row 5, column 1).
    // Every key a family matches (rows `fields.json` lists too), so listed
    // and supplemented rows of one family share their pattern.
    let families: Vec<String> = def
        .boxes
        .iter()
        .filter(|entry| family_parts(&entry.key).is_some())
        .map(|entry| entry.key.clone())
        .collect();
    let split = |key: &str| -> Option<(String, usize, String)> {
        let mut found = families.iter().filter_map(|family| {
            let row = family_match(&family_parts(family)?, key, None)?;
            Some((family.clone(), row, String::new()))
        });
        if let Some(first) = found.next()
            && found.next().is_none()
        {
            return Some(first);
        }
        split_index(key)
            .map(|(prefix, number, suffix)| (prefix.to_string(), number, suffix.to_string()))
    };
    let mut patterns: BTreeMap<RowPattern, (usize, BoundBox)> = BTreeMap::new();
    for (key, bound) in &def.bindings {
        let (Some((prefix, number, suffix)), Some((row, parts))) =
            (split(key), row_of(&bound.binding))
        else {
            continue;
        };
        let [(list, field)] = parts.as_slice() else {
            continue;
        };
        let offset = number as isize - row as isize;
        patterns
            .entry((prefix, suffix, offset, list.clone(), field.clone()))
            .and_modify(|entry| entry.0 += 1)
            .or_insert((1, bound.clone()));
    }
    // Only boxes no probed row reached; a box a probe moved has its reason.
    let unbound: Vec<String> = def
        .unmapped
        .iter()
        .filter(|(key, reason)| !key.contains('{') && reason.starts_with("not written"))
        .map(|(key, _)| key.clone())
        .collect();
    for key in unbound {
        let Some((prefix, number, suffix)) = split(&key) else {
            continue;
        };
        let found = patterns.iter().find(|((p, s, offset, _, _), (seen, _))| {
            *p == prefix && *s == suffix && *seen >= 2 && number as isize >= *offset
        });
        let Some(((_, _, offset, list, field), (_, template))) = found else {
            continue;
        };
        let row = (number as isize - offset) as usize;
        let path = format!("{list}/{row}/{field}");
        let binding = match &template.binding {
            Binding::Text {
                nullable,
                date,
                part,
                ..
            } => Binding::Text {
                path,
                nullable: *nullable,
                date: *date,
                part: *part,
            },
            Binding::Amount { nullable, .. } => Binding::Amount {
                path,
                nullable: *nullable,
            },
            Binding::Whole { nullable, .. } => Binding::Whole {
                path,
                nullable: *nullable,
            },
            Binding::Choice { .. } => continue,
        };
        def.unmapped.remove(&key);
        def.bindings.insert(
            key,
            BoundBox {
                binding,
                lock: template.lock,
            },
        );
    }
}

/// Bind `key` from the evidence, or say why it cannot be.
fn decide(
    def: &AgentFormDef,
    evidence: &Evidence,
    profile: &BTreeSet<String>,
    key: &str,
) -> Result<BoundBox, String> {
    let probed = decide_probed(def, evidence, profile, key);
    let slot = POPUP_SLOTS.iter().find(|(form, prefix, _)| {
        *form == def.form_code
            && key
                .strip_prefix(prefix)
                .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
    });
    let Some((_, _, reason)) = slot else {
        return probed;
    };
    let seen = match &probed {
        Ok(bound) => format!(
            "the probe tied it to {}",
            super::binding_paths(&bound.binding)
                .into_iter()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Err(why) => format!("probe: {why}"),
    };
    Err(format!("{reason}; {seen}"))
}

fn decide_probed(
    def: &AgentFormDef,
    evidence: &Evidence,
    profile: &BTreeSet<String>,
    key: &str,
) -> Result<BoundBox, String> {
    let lock_for = |paths: &[&str]| -> Option<Lock> {
        if paths
            .iter()
            .any(|path| evidence.identity_paths.contains(*path))
        {
            Some(Lock::Identity)
        } else if paths.iter().any(|path| is_election(path)) {
            Some(Lock::Election)
        } else if paths.iter().any(|path| {
            evidence
                .strings
                .get(*path)
                .is_some_and(|values| values.iter().any(|value| profile.contains(value)))
        }) {
            Some(Lock::Profile)
        } else {
            None
        }
    };
    let echoes: Option<Vec<&(String, ValueKind, bool)>> = evidence.echoes.get(key).map(|echoes| {
        echoes
            .iter()
            .filter(|(path, _, _)| !evidence.misses.contains(&(key.to_string(), path.clone())))
            .collect()
    });
    if let Some(echoes) = echoes.filter(|echoes| !echoes.is_empty()) {
        let (path, value_kind, nullable) = echoes
            .into_iter()
            .min_by_key(|(path, value_kind, _)| {
                (
                    !evidence
                        .real_echoes
                        .contains(&(key.to_string(), path.clone())),
                    evidence.echo_count.get(path).copied(),
                    path.clone(),
                    // One path echoed as a whole date by one sample and as
                    // plain text by another (a row a sample leaves blank):
                    // a box of DATED_ROWS_KEEP_THE_DATE keeps the date.
                    // Elsewhere the first echo stays, as before.
                    keeps_the_date(key) && matches!(value_kind, ValueKind::Text(None)),
                )
            })
            .cloned()
            .expect("non-empty echo set");
        // Another field's amount or text also moves this box: it is a sum
        // or a combination (Total Remitted = Tax + Penalty), not an entry,
        // unless the other field only clears it (`GATED_ENTRY`).
        if !GATED_ENTRY.iter().any(|(k, _)| *k == key)
            && !CASE_FOLLOWS_ANOTHER_FIELD.iter().any(|(k, _)| *k == key)
            && let Some((_, other)) = evidence
                .misses
                .iter()
                .find(|(missed, other)| missed == key && *other != path)
        {
            return Err(format!(
                "derived by the app: shows {path} combined with {other}"
            ));
        }
        let lock = lock_for(&[path.as_str()])
            .filter(|lock| *lock != Lock::Profile || !NOT_PROFILE.iter().any(|(k, _)| *k == key))
            .or_else(|| {
                PROFILE_FACT
                    .iter()
                    .any(|(k, _)| *k == key)
                    .then_some(Lock::Profile)
            });
        let binding = match value_kind {
            ValueKind::Text(date) => Binding::Text {
                path,
                nullable,
                date: date.map(DateFormatKey::format),
                part: None,
            },
            ValueKind::DatePart(date, part) => Binding::Text {
                path,
                nullable,
                date: Some(date.format()),
                part: Some(part),
            },
            ValueKind::Amount => Binding::Amount { path, nullable },
            ValueKind::Whole => Binding::Whole { path, nullable },
        };
        return Ok(BoundBox { binding, lock });
    }
    let choice_box = def.box_def(key).is_some_and(is_choice_box);
    if let Some(mut options) = choice_binding(evidence, key, choice_box)? {
        let ours = |form: &str, k: &str| form == def.form_code && k == key;
        for (_, _, value, path, set, _) in CHOICE_OPTION_SETS
            .iter()
            .filter(|(form, k, ..)| ours(form, k))
        {
            if let Some(option) = options.iter_mut().find(|option| option.value == *value) {
                option
                    .set
                    .insert((*path).to_string(), Value::String((*set).to_string()));
            }
        }
        for (_, _, value, path, set, _) in CHOICE_OPTION_BOOL_SETS
            .iter()
            .filter(|(form, k, ..)| ours(form, k))
        {
            if let Some(option) = options.iter_mut().find(|option| option.value == *value) {
                option.set.insert((*path).to_string(), Value::Bool(*set));
            }
        }
        for (_, _, prefix, _) in CHOICE_PATHS_WITHIN
            .iter()
            .filter(|(form, k, ..)| ours(form, k))
        {
            for option in &mut options {
                option.set.retain(|path, _| path.starts_with(prefix));
            }
        }
        let paths: BTreeSet<&str> = options
            .iter()
            .flat_map(|option| option.set.keys().map(String::as_str))
            .collect();
        let paths: Vec<&str> = paths.into_iter().collect();
        return Ok(BoundBox {
            lock: lock_for(&paths),
            binding: Binding::Choice { options },
        });
    }
    if key.contains("{N") {
        return Err(
            "a row family pattern, not a box: each row is its own numbered key".to_string(),
        );
    }
    if SHUT_ON_OFFICIAL_PAGE
        .iter()
        .any(|(form, shut)| *form == def.form_code && *shut == key)
    {
        return Err(SHUT_REASON.to_string());
    }
    Err(if !evidence.present.contains(key) {
        "not written by this app's field map for any sample draft".to_string()
    } else if let Some(paths) = evidence.touched.get(key) {
        let named: Vec<&str> = paths.iter().take(3).map(String::as_str).collect();
        format!(
            "derived by the app: changes with {}{} but never shows one field's value",
            named.join(", "),
            if paths.len() > 3 { ", ..." } else { "" }
        )
    } else {
        "written by the app without a draft field (a constant or the period)".to_string()
    })
}

/// Boxes the app's field map writes that `fields.json` does not list, when
/// the package describes another revision than the one the app serializes
/// (fewer than half of the written keys are in it). Labels come from the
/// key (`frm1700:txtPg1I1Year` -> `Page 1 Item 1 Year`).
pub fn supplement_from_field_map(
    kind: AgentFormKind,
    seeds: &[Value],
    boxes: &[BoxDef],
) -> Vec<BoxDef> {
    let mut written: BTreeMap<String, String> = BTreeMap::new();
    for seed in seeds {
        if let Some((_, map)) = probe_eval(kind, seed) {
            written.extend(map);
        }
    }
    let listed: BTreeSet<&str> = boxes.iter().map(|entry| entry.key.as_str()).collect();
    // A printed box the official program injects at run time.
    let injected: Vec<BoxDef> = PRINTED_NOT_IN_CORPUS
        .iter()
        .filter(|(key, _, _, _)| written.contains_key(*key) && !listed.contains(key))
        .map(|(key, item, caption, _)| BoxDef {
            label: (*caption).to_string(),
            item: (*item).to_string(),
            kind: "runtime-generated-select".to_string(),
            computed: false,
            required: "required".to_string(),
            options: Vec::new(),
            layout_only: false,
            group: None,
            radio_group: false,
            members: Vec::new(),
            choice: String::new(),
            choice_value: String::new(),
            family: None,
            key: (*key).to_string(),
        })
        .collect();
    let known = written
        .keys()
        .filter(|key| listed.contains(key.as_str()))
        .count();
    if written.is_empty() || known * 2 >= written.len() {
        return injected;
    }
    let taken: BTreeSet<String> = injected.iter().map(|entry| entry.key.clone()).collect();
    written
        .into_iter()
        .filter(|(key, _)| !listed.contains(key.as_str()) && !taken.contains(key))
        .map(|(key, value)| {
            let control = key.rsplit(':').next().unwrap_or(&key).to_string();
            let flag = value == "true" || value == "false";
            BoxDef {
                label: label_from_key(&control),
                item: String::new(),
                kind: if !flag {
                    "text".to_string()
                } else if control.starts_with("rdo") {
                    "radio".to_string()
                } else {
                    "checkbox".to_string()
                },
                computed: false,
                required: "optional".to_string(),
                options: Vec::new(),
                layout_only: true,
                group: None,
                radio_group: false,
                members: Vec::new(),
                choice: String::new(),
                choice_value: String::new(),
                family: None,
                key,
            }
        })
        .chain(injected)
        .collect()
}

/// One piece of a row family key: `txtAmt{N>=0}`, `txtPg2Sc3_{N>=1}Col2`,
/// `txtSched1ATC{n>=28}`, `txtTaxBase{index}`, `..._1A{EX|SP}{N>=1}`.
enum FamilyPart<'a> {
    Text(&'a str),
    /// The row number, at least this.
    Number(usize),
    OneOf(Vec<&'a str>),
}

/// The pieces of a row family key, or `None` when it has no row number.
fn family_parts(template: &str) -> Option<Vec<FamilyPart<'_>>> {
    let mut parts = Vec::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        if open > 0 {
            parts.push(FamilyPart::Text(&rest[..open]));
        }
        let close = open + rest[open..].find('}')?;
        let inner = &rest[open + 1..close];
        let lower = inner.to_ascii_lowercase();
        if let Some(min) = lower.strip_prefix("n>=") {
            parts.push(FamilyPart::Number(min.parse().ok()?));
        } else if lower == "index" {
            parts.push(FamilyPart::Number(0));
        } else if inner.contains('|') {
            parts.push(FamilyPart::OneOf(inner.split('|').collect()));
        } else {
            return None;
        }
        rest = &rest[close + 1..];
    }
    if !rest.is_empty() {
        parts.push(FamilyPart::Text(rest));
    }
    parts
        .iter()
        .any(|part| matches!(part, FamilyPart::Number(_)))
        .then_some(parts)
}

/// `Some(row)` when `key` is a row of the family (the first number in it).
fn family_match(parts: &[FamilyPart], key: &str, row: Option<usize>) -> Option<usize> {
    let Some((first, rest)) = parts.split_first() else {
        return if key.is_empty() { row } else { None };
    };
    match first {
        FamilyPart::Text(text) => family_match(rest, key.strip_prefix(text)?, row),
        FamilyPart::OneOf(options) => options
            .iter()
            .find_map(|option| family_match(rest, key.strip_prefix(option)?, row)),
        FamilyPart::Number(min) => {
            let digits = key.bytes().take_while(u8::is_ascii_digit).count();
            (1..=digits).rev().find_map(|len| {
                if len > 1 && key.starts_with('0') {
                    return None;
                }
                let number: usize = key[..len].parse().ok()?;
                if number < *min {
                    return None;
                }
                family_match(rest, &key[len..], row.or(Some(number)))
            })
        }
    }
}

/// Rows past a list's own (and past [`PROBE_ROWS`]) that
/// [`supplement_row_families`] grows it by, so a form whose list starts
/// long (2200P's 27 rows) still gets rows the filer can add.
const FAMILY_ROWS: usize = 10;

/// `seed` with each non-empty list grown by `extra` rows past its own
/// length and past [`PROBE_ROWS`], repeating its last row. A list is grown
/// only where the draft still parses with it: a fixed-length field
/// (1702RT's three address lines) is a JSON array too.
fn grow_rows_past(kind: AgentFormKind, seed: &Value, extra: usize) -> Value {
    let mut grown = seed.clone();
    let mut lists = Vec::new();
    collect_lists(seed, "", 0, &mut lists);
    for (path, _) in lists {
        let Some(Value::Array(items)) = grown.pointer(&path) else {
            continue;
        };
        let Some(last) = items.last().cloned() else {
            continue;
        };
        let target = (items.len() + extra).max(PROBE_ROWS);
        let mut trial = grown.clone();
        if let Some(Value::Array(items)) = trial.pointer_mut(&path) {
            while items.len() < target {
                items.push(last.clone());
            }
        }
        if parse_error(kind, &trial).is_none() {
            grown = trial;
        }
    }
    grown
}

/// The rows of each row family the app's field map writes, as boxes of
/// their own. The official page builds these rows in script, so
/// `fields.json` lists only the family (`txtAmt{N>=0}`); the probe needs
/// each row's key to bind it. Rows come from every sample and from the
/// sample with its lists grown ([`grow_rows_past`]). A key two
/// families both match is left out.
pub fn supplement_row_families(
    kind: AgentFormKind,
    seeds: &[Value],
    boxes: &[BoxDef],
) -> Vec<BoxDef> {
    let families: Vec<(&BoxDef, Vec<FamilyPart>)> = boxes
        .iter()
        .filter_map(|entry| family_parts(&entry.key).map(|parts| (entry, parts)))
        .collect();
    if families.is_empty() {
        return Vec::new();
    }
    let mut written: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut note = |map: BTreeMap<String, String>| {
        for (key, value) in map {
            written.entry(key).or_default().insert(value);
        }
    };
    for seed in seeds {
        let Some((computed, map)) = probe_eval(kind, seed) else {
            continue;
        };
        note(map);
        let grown = grow_rows_past(kind, &computed, FAMILY_ROWS);
        if grown != computed
            && let Some((_, map)) = probe_eval(kind, &grown)
        {
            note(map);
        }
    }
    let listed: BTreeSet<&str> = boxes.iter().map(|entry| entry.key.as_str()).collect();
    written
        .into_iter()
        .filter(|(key, _)| !listed.contains(key.as_str()))
        // A row tick box the app always writes unticked (a Delete
        // selector) is a constant, not a box: a probe flipping another
        // field could only tie it to that field by coincidence.
        .filter(|(_, values)| values.iter().any(|value| value != "false"))
        .filter_map(|(key, values)| {
            let mut found = families
                .iter()
                .filter_map(|(family, parts)| Some((*family, family_match(parts, &key, None)?)));
            let (family, row) = found.next()?;
            if found.next().is_some() {
                return None;
            }
            let control = key.rsplit(':').next().unwrap_or(&key).to_string();
            let flag = values
                .iter()
                .all(|value| value == "true" || value == "false");
            Some(BoxDef {
                label: format!("{}, row {row}", family.label),
                item: family.item.clone(),
                kind: if !flag {
                    "text".to_string()
                } else if control.starts_with("rdo") {
                    "radio".to_string()
                } else {
                    "checkbox".to_string()
                },
                computed: family.computed,
                required: family.required.clone(),
                options: family.options.clone(),
                layout_only: false,
                group: None,
                radio_group: false,
                members: Vec::new(),
                choice: String::new(),
                choice_value: String::new(),
                family: Some(family.key.clone()),
                key,
            })
        })
        .collect()
}

fn label_from_key(control: &str) -> String {
    let body = control.trim_start_matches(|c: char| c.is_ascii_lowercase());
    let mut words: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut previous: Option<char> = None;
    for c in body.chars() {
        let boundary = match previous {
            Some(p) => {
                (c.is_ascii_uppercase() && !p.is_ascii_uppercase())
                    || (c.is_ascii_digit() != p.is_ascii_digit())
                    || c == '_'
            }
            None => false,
        };
        if boundary && !current.is_empty() {
            words.push(std::mem::take(&mut current));
        }
        if c != '_' {
            current.push(c);
        }
        previous = Some(c);
    }
    if !current.is_empty() {
        words.push(current);
    }
    let mut out: Vec<String> = Vec::new();
    let mut iter = words.into_iter().peekable();
    while let Some(word) = iter.next() {
        let number = iter
            .peek()
            .filter(|next| next.chars().all(|c| c.is_ascii_digit()));
        match (word.as_str(), number) {
            ("Pg", Some(_)) => out.push("Page".to_string()),
            ("I", Some(_)) => out.push("Item".to_string()),
            ("Pt", Some(_)) => out.push("Part".to_string()),
            _ => out.push(word),
        }
    }
    out.join(" ")
}

/// Options of a choice box, or why it has none.
fn choice_binding(
    evidence: &Evidence,
    key: &str,
    choice_box: bool,
) -> Result<Option<Vec<ChoiceOption>>, String> {
    let Some(observations) = evidence.choices.get(key) else {
        return Ok(None);
    };
    // A text box that only appears with a flag is not that flag's answer.
    if !choice_box
        && observations
            .iter()
            .any(|obs| !matches!(obs.official.as_str(), "true" | "false"))
    {
        return Ok(None);
    }
    let paths: BTreeSet<&String> = observations
        .iter()
        .filter_map(|obs| obs.change.as_ref().map(|(path, _)| path))
        .collect();
    if paths.is_empty() {
        return Ok(None);
    }
    if paths.len() > 6 {
        return Err(format!(
            "derived by the app from {} flags or options, not one answer",
            paths.len()
        ));
    }
    let mut best: BTreeMap<&str, (&ChoiceObs, (bool, usize, usize))> = BTreeMap::new();
    for obs in observations {
        let changes = usize::from(obs.change.is_some());
        let score = (!obs.answered, changes, obs.seed);
        best.entry(obs.official.as_str())
            .and_modify(|current| {
                if score < current.1 {
                    *current = (obs, score);
                }
            })
            .or_insert((obs, score));
    }
    if best.len() < 2 {
        return Ok(None);
    }
    // One path that alone decides the box answers it: its own changes (with
    // the seeds) produce every value the box takes, each from a different
    // value of that path. The flags that merely open the box (1701MS Item
    // 31's checkboxes open only for some Item 12 tax options) stay out of its
    // options, so ticking it never rewrites them. When several paths decide
    // it, the one that moves the fewest other boxes is the answer (a gate
    // opens many); a tie keeps every path.
    let deciding = |path: &String| -> Option<Vec<ChoiceOption>> {
        let mut options = Vec::new();
        let mut taken: Vec<&Value> = Vec::new();
        for official in best.keys() {
            let obs = observations
                .iter()
                .filter(|obs| obs.official == *official)
                .filter(|obs| {
                    obs.change
                        .as_ref()
                        .is_none_or(|(changed, _)| changed == path)
                })
                .min_by_key(|obs| (!obs.answered, usize::from(obs.change.is_some()), obs.seed))?;
            let value = match &obs.change {
                Some((_, value)) => value,
                None => evidence.seed_leaves.get(&obs.seed)?.get(path)?,
            };
            if taken.contains(&value) {
                return None;
            }
            taken.push(value);
            options.push(ChoiceOption {
                value: official.to_string(),
                set: BTreeMap::from([(path.clone(), value.clone())]),
            });
        }
        Some(options)
    };
    if paths.len() > 1 {
        let mut deciders: Vec<(usize, Vec<ChoiceOption>)> = paths
            .iter()
            .filter_map(|path| {
                let moved = evidence.gates.get(*path).map_or(0, BTreeSet::len);
                deciding(path).map(|options| (moved, options))
            })
            .collect();
        deciders.sort_by_key(|(moved, _)| *moved);
        if let Some((fewest, _)) = deciders.first()
            && deciders.iter().filter(|(moved, _)| moved == fewest).count() == 1
        {
            return Ok(Some(deciders.remove(0).1));
        }
    }
    let options = best
        .into_iter()
        .map(|(official, (obs, _))| {
            let empty = BTreeMap::new();
            let seed = evidence.seed_leaves.get(&obs.seed).unwrap_or(&empty);
            let mut set: BTreeMap<String, Value> = paths
                .iter()
                .filter_map(|path| {
                    seed.get(*path)
                        .map(|value| ((*path).clone(), value.clone()))
                })
                .collect();
            if let Some((path, value)) = &obs.change {
                set.insert(path.clone(), value.clone());
            }
            ChoiceOption {
                value: official.to_string(),
                set,
            }
        })
        .collect();
    Ok(Some(options))
}
