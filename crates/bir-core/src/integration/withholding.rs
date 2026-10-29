use super::transactions::Invoice;
use crate::forms::FilingStatus;
use crate::forms::form_2307::{Form2307Draft, Form2307IncomePayment};

pub struct WithholdingConfig {
    pub atc_code: String,
    pub atc_description: String,
    pub withholding_rate: f64,
}

pub struct WithholdingEngine;

impl WithholdingEngine {
    /// Generates a Form 2307 (Certificate of Creditable Tax Withheld at Source)
    /// based on the *accrual* of the expense (when the invoice is issued).
    /// Under RR 4-2024 (EOPT), withholding tax is due when the income payment
    /// becomes payable, demandable, or legally enforceable, which usually coincides
    /// with the invoice issuance date for accrual basis taxpayers, rather than
    /// the date of actual payment.
    pub fn process_accrual_withholding(
        invoice: &Invoice,
        payor_tin: &str,
        payor_name: &str,
        config: &WithholdingConfig,
    ) -> Option<Form2307Draft> {
        // If there's no sales amount, no withholding is needed
        if invoice.sales_amount <= 0.0 {
            return None;
        }

        let tax_withheld = invoice.sales_amount * config.withholding_rate;

        let now = chrono::Utc::now().to_rfc3339();

        let mut draft = Form2307Draft {
            id: None,
            payee_tin: invoice.party_tin.clone(),
            payee_name: invoice.party_name.clone(),
            payor_tin: payor_tin.to_string(),
            payor_name: payor_name.to_string(),
            issue_date: invoice.issue_date,
            income_payments: vec![Form2307IncomePayment {
                atc: config.atc_code.clone(),
                description: config.atc_description.clone(),
                amount: invoice.sales_amount,
                tax_withheld,
            }],
            total_amount: 0.0,
            total_tax_withheld: 0.0,
            status: FilingStatus::Draft,
            created_at: now.clone(),
            updated_at: now,
        };

        draft.recompute();

        Some(draft)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integration::transactions::InvoiceStatus;

    #[test]
    fn test_accrual_withholding_generation() {
        let invoice = Invoice {
            invoice_no: "EXP-001".to_string(),
            issue_date: chrono::NaiveDate::from_ymd_opt(2026, 6, 1).unwrap(),
            payment_terms_days: 30,
            party_tin: "999-999-999-000".to_string(),
            party_name: "Supplier Inc".to_string(),
            sales_amount: 50_000.0,
            vat_amount: 6_000.0,
            description: "Office Supplies".to_string(),
            status: InvoiceStatus::Unpaid, // It is unpaid, but withholding generates anyway!
            is_service: false,
        };

        let config = WithholdingConfig {
            atc_code: "WI158".to_string(),
            atc_description: "Professional Services".to_string(),
            withholding_rate: 0.10,
        };

        let form2307 = WithholdingEngine::process_accrual_withholding(
            &invoice,
            "123-456-789-000",
            "My Company",
            &config,
        )
        .unwrap();

        assert_eq!(form2307.payee_name, "Supplier Inc");
        assert_eq!(form2307.payor_name, "My Company");
        assert_eq!(form2307.issue_date, invoice.issue_date);
        assert_eq!(form2307.total_amount, 50_000.0);
        assert_eq!(form2307.total_tax_withheld, 5_000.0); // 10%
    }
}
