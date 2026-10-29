use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum InvoiceStatus {
    Unpaid,
    Paid(NaiveDate),
    Lapsed,               // Uncollected beyond agreed payment terms
    Recovered(NaiveDate), // Previously lapsed, now collected
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Invoice {
    pub invoice_no: String,
    pub issue_date: NaiveDate,
    pub payment_terms_days: u32,
    pub party_tin: String,
    pub party_name: String,
    pub sales_amount: f64,
    pub vat_amount: f64,
    pub description: String,
    pub status: InvoiceStatus,
    pub is_service: bool, // If true, EOPT accrual VAT rules apply
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VatEventType {
    OutputVatRecognized,
    OutputVatCreditLapsed,
    OutputVatRecovered,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VatEvent {
    pub event_type: VatEventType,
    pub amount: f64,
    pub date: NaiveDate,
    pub reference_invoice: String,
}

impl Invoice {
    /// Validates the invoice against EOPT substantiation requirements.
    /// Official Receipts are no longer the primary document for services;
    /// an Invoice must contain Sales Amount, VAT Amount, Name, TIN, Description, and Date.
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();

        if self.party_tin.trim().is_empty() {
            errors.push("Invoice must contain buyer's TIN".to_string());
        }
        if self.party_name.trim().is_empty() {
            errors.push("Invoice must contain buyer's Name".to_string());
        }
        if self.sales_amount <= 0.0 {
            errors.push("Sales amount must be greater than zero".to_string());
        }
        if self.vat_amount < 0.0 {
            errors.push("VAT amount cannot be negative".to_string());
        }
        if self.description.trim().is_empty() {
            errors.push("Invoice must contain a transaction description".to_string());
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Derives the Output VAT events based on the invoice's lifecycle,
    /// complying with EOPT's accrual-based VAT recognition for services.
    pub fn derive_output_vat_events(&self) -> Vec<VatEvent> {
        let mut events = Vec::new();

        if self.vat_amount <= 0.0 {
            return events;
        }

        // Under EOPT, Output VAT is recognized upon issuance for both goods and services
        events.push(VatEvent {
            event_type: VatEventType::OutputVatRecognized,
            amount: self.vat_amount,
            date: self.issue_date,
            reference_invoice: self.invoice_no.clone(),
        });

        // Handle uncollected receivables for services
        if self.is_service {
            match self.status {
                InvoiceStatus::Lapsed => {
                    // Credit for uncollected receivable
                    let lapse_date =
                        self.issue_date + chrono::Duration::days(self.payment_terms_days as i64);
                    events.push(VatEvent {
                        event_type: VatEventType::OutputVatCreditLapsed,
                        amount: self.vat_amount,
                        date: lapse_date,
                        reference_invoice: self.invoice_no.clone(),
                    });
                }
                InvoiceStatus::Recovered(recovery_date) => {
                    // First it lapsed...
                    let lapse_date =
                        self.issue_date + chrono::Duration::days(self.payment_terms_days as i64);
                    events.push(VatEvent {
                        event_type: VatEventType::OutputVatCreditLapsed,
                        amount: self.vat_amount,
                        date: lapse_date,
                        reference_invoice: self.invoice_no.clone(),
                    });

                    // ...then it was recovered
                    events.push(VatEvent {
                        event_type: VatEventType::OutputVatRecovered,
                        amount: self.vat_amount,
                        date: recovery_date,
                        reference_invoice: self.invoice_no.clone(),
                    });
                }
                _ => {}
            }
        }

        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mock_invoice() -> Invoice {
        Invoice {
            invoice_no: "INV-001".to_string(),
            issue_date: NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            payment_terms_days: 90,
            party_tin: "123-456-789-000".to_string(),
            party_name: "Juan Dela Cruz".to_string(),
            sales_amount: 10_000.0,
            vat_amount: 1_200.0,
            description: "Consulting Services".to_string(),
            status: InvoiceStatus::Unpaid,
            is_service: true,
        }
    }

    #[test]
    fn test_invoice_validation() {
        let mut inv = mock_invoice();
        assert!(inv.validate().is_ok());

        inv.party_tin = "".to_string();
        inv.sales_amount = 0.0;
        let errs = inv.validate().unwrap_err();
        assert_eq!(errs.len(), 2);
    }

    #[test]
    fn test_vat_recognition_unpaid() {
        let inv = mock_invoice();
        let events = inv.derive_output_vat_events();

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, VatEventType::OutputVatRecognized);
        assert_eq!(events[0].amount, 1_200.0);
    }

    #[test]
    fn test_vat_recognition_lapsed() {
        let mut inv = mock_invoice();
        inv.status = InvoiceStatus::Lapsed;

        let events = inv.derive_output_vat_events();

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_type, VatEventType::OutputVatRecognized);
        assert_eq!(events[1].event_type, VatEventType::OutputVatCreditLapsed);

        // 90 days after Jan 1 is April 1 (in a non-leap year)
        assert_eq!(events[1].date, NaiveDate::from_ymd_opt(2026, 4, 1).unwrap());
    }

    #[test]
    fn test_vat_recognition_recovered() {
        let mut inv = mock_invoice();
        let recovery_date = NaiveDate::from_ymd_opt(2026, 5, 1).unwrap();
        inv.status = InvoiceStatus::Recovered(recovery_date);

        let events = inv.derive_output_vat_events();

        assert_eq!(events.len(), 3);
        assert_eq!(events[0].event_type, VatEventType::OutputVatRecognized);
        assert_eq!(events[1].event_type, VatEventType::OutputVatCreditLapsed);
        assert_eq!(events[2].event_type, VatEventType::OutputVatRecovered);
        assert_eq!(events[2].date, recovery_date);
    }
}
