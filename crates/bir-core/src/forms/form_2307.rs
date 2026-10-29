use super::FilingStatus;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Form2307IncomePayment {
    pub atc: String,
    pub description: String,
    pub amount: f64,
    pub tax_withheld: f64,
}

/// Certificate of Creditable Tax Withheld at Source
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Form2307Draft {
    pub id: Option<i64>,
    pub payee_tin: String,
    pub payee_name: String,
    pub payor_tin: String,
    pub payor_name: String,

    pub issue_date: chrono::NaiveDate,
    pub income_payments: Vec<Form2307IncomePayment>,

    pub total_amount: f64,
    pub total_tax_withheld: f64,

    pub status: FilingStatus,
    pub created_at: String,
    pub updated_at: String,
}

impl Form2307Draft {
    pub fn recompute(&mut self) {
        self.total_amount = self.income_payments.iter().map(|p| p.amount).sum();
        self.total_tax_withheld = self.income_payments.iter().map(|p| p.tax_withheld).sum();
        self.updated_at = chrono::Utc::now().to_rfc3339();
    }
}
