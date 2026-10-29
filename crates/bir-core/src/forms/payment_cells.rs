//! The "Details of Payment" boxes some pages leave open to the filer
//! (1801 v2018, 2200A and 2200T v2020: `cashBank` ... `othersAmt`). The
//! page uploads them as typed (raw codec, no handler), so a draft keeps the
//! text as typed and writes it back to the same controls.

use serde::{Deserialize, Serialize};

/// Cash/bank debit memo, check, tax debit memo and others rows.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PaymentCells {
    pub cash_bank: String,
    pub cash_number: String,
    pub cash_date: String,
    pub cash_amount: String,
    pub check_bank: String,
    pub check_number: String,
    pub check_date: String,
    pub check_amount: String,
    pub tax_number: String,
    pub tax_date: String,
    pub tax_amount: String,
    pub others_details: String,
    pub others_bank: String,
    pub others_number: String,
    pub others_date: String,
    pub others_amount: String,
}

impl PaymentCells {
    /// `(control id, value)` for each box. `prefix` is the form's control
    /// prefix (`frm2200Av2020`); `tax_number_key` is the tax debit memo
    /// number's full id, which 1801 spells `frm1801v2018taxkNum`.
    pub fn controls(&self, prefix: &str, tax_number_key: &str) -> Vec<(String, String)> {
        let key = |name: &str| format!("{prefix}:{name}");
        let text = |value: &str| value.trim().to_string();
        vec![
            (key("cashBank"), text(&self.cash_bank)),
            (key("cashNum"), text(&self.cash_number)),
            (key("cashDate"), text(&self.cash_date)),
            (key("cashAmt"), text(&self.cash_amount)),
            (key("checkBank"), text(&self.check_bank)),
            (key("checkNum"), text(&self.check_number)),
            (key("checkDate"), text(&self.check_date)),
            (key("checkAmt"), text(&self.check_amount)),
            (tax_number_key.to_string(), text(&self.tax_number)),
            (key("taxDate"), text(&self.tax_date)),
            (key("taxAmt"), text(&self.tax_amount)),
            (key("othersDetails"), text(&self.others_details)),
            (key("othersBank"), text(&self.others_bank)),
            (key("othersNum"), text(&self.others_number)),
            (key("othersDate"), text(&self.others_date)),
            (key("othersAmt"), text(&self.others_amount)),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_box_is_written_under_its_official_id() {
        let cells = PaymentCells {
            cash_bank: " BDO ".into(),
            tax_number: "TDM-1".into(),
            others_amount: "1,000.00".into(),
            ..Default::default()
        };
        let controls = cells.controls("frm1801v2018", "frm1801v2018taxkNum");
        assert_eq!(controls.len(), 16);
        assert_eq!(controls[0], ("frm1801v2018:cashBank".into(), "BDO".into()));
        assert_eq!(controls[8], ("frm1801v2018taxkNum".into(), "TDM-1".into()));
        assert_eq!(controls[15].1, "1,000.00");
    }
}
