use super::taxpayer::TaxpayerClass;
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterestRateRule {
    pub taxpayer_class: TaxpayerClass,
    pub annual_rate: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SurchargeRateRule {
    pub taxpayer_class: TaxpayerClass,
    pub is_fraud: bool,
    pub rate: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PenaltyConfig {
    pub interest_rates: Vec<InterestRateRule>,
    pub surcharge_rates: Vec<SurchargeRateRule>,
    pub train_interest_effective_date: NaiveDate,
    pub eopt_micro_small_effective_date: NaiveDate,
    pub pre_train_interest_rate: f64,
    pub micro_small_compromise_multiplier: f64,
    // In a real system, compromise tables would also be here.
    // For now, we encapsulate them in compromise.rs to keep it simple,
    // but the architecture allows for loading them.
}

impl PenaltyConfig {
    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).expect("valid penalty rule effective date")
    }

    pub fn default_rules() -> Self {
        Self {
            interest_rates: vec![
                InterestRateRule {
                    taxpayer_class: TaxpayerClass::Micro,
                    annual_rate: 0.06,
                },
                InterestRateRule {
                    taxpayer_class: TaxpayerClass::Small,
                    annual_rate: 0.06,
                },
                InterestRateRule {
                    taxpayer_class: TaxpayerClass::Medium,
                    annual_rate: 0.12,
                },
                InterestRateRule {
                    taxpayer_class: TaxpayerClass::Large,
                    annual_rate: 0.12,
                },
                InterestRateRule {
                    taxpayer_class: TaxpayerClass::Regular,
                    annual_rate: 0.12,
                },
            ],
            surcharge_rates: vec![
                // Non-fraud
                SurchargeRateRule {
                    taxpayer_class: TaxpayerClass::Micro,
                    is_fraud: false,
                    rate: 0.10,
                },
                SurchargeRateRule {
                    taxpayer_class: TaxpayerClass::Small,
                    is_fraud: false,
                    rate: 0.10,
                },
                SurchargeRateRule {
                    taxpayer_class: TaxpayerClass::Medium,
                    is_fraud: false,
                    rate: 0.25,
                },
                SurchargeRateRule {
                    taxpayer_class: TaxpayerClass::Large,
                    is_fraud: false,
                    rate: 0.25,
                },
                SurchargeRateRule {
                    taxpayer_class: TaxpayerClass::Regular,
                    is_fraud: false,
                    rate: 0.25,
                },
                // Fraud
                SurchargeRateRule {
                    taxpayer_class: TaxpayerClass::Micro,
                    is_fraud: true,
                    rate: 0.50,
                },
                SurchargeRateRule {
                    taxpayer_class: TaxpayerClass::Small,
                    is_fraud: true,
                    rate: 0.50,
                },
                SurchargeRateRule {
                    taxpayer_class: TaxpayerClass::Medium,
                    is_fraud: true,
                    rate: 0.50,
                },
                SurchargeRateRule {
                    taxpayer_class: TaxpayerClass::Large,
                    is_fraud: true,
                    rate: 0.50,
                },
                SurchargeRateRule {
                    taxpayer_class: TaxpayerClass::Regular,
                    is_fraud: true,
                    rate: 0.50,
                },
            ],
            train_interest_effective_date: Self::date(2018, 1, 1),
            eopt_micro_small_effective_date: Self::date(2024, 1, 22),
            pre_train_interest_rate: 0.20,
            micro_small_compromise_multiplier: 0.50,
        }
    }

    pub fn is_micro_or_small_reduction_applicable(
        &self,
        class: TaxpayerClass,
        due_date: NaiveDate,
    ) -> bool {
        matches!(class, TaxpayerClass::Micro | TaxpayerClass::Small)
            && due_date >= self.eopt_micro_small_effective_date
    }

    pub fn get_interest_rate(&self, class: TaxpayerClass) -> f64 {
        self.interest_rates
            .iter()
            .find(|r| r.taxpayer_class == class)
            .map(|r| r.annual_rate)
            .unwrap_or(0.12)
    }

    pub fn get_surcharge_rate(&self, class: TaxpayerClass, is_fraud: bool) -> f64 {
        self.surcharge_rates
            .iter()
            .find(|r| r.taxpayer_class == class && r.is_fraud == is_fraud)
            .map(|r| r.rate)
            .unwrap_or(if is_fraud { 0.50 } else { 0.25 })
    }

    pub fn get_surcharge_rate_for_due_date(
        &self,
        class: TaxpayerClass,
        is_fraud: bool,
        due_date: NaiveDate,
    ) -> f64 {
        if self.is_micro_or_small_reduction_applicable(class, due_date) {
            return self.get_surcharge_rate(class, is_fraud);
        }

        if is_fraud { 0.50 } else { 0.25 }
    }

    pub fn get_compromise_multiplier_for_due_date(
        &self,
        class: TaxpayerClass,
        due_date: NaiveDate,
    ) -> f64 {
        if self.is_micro_or_small_reduction_applicable(class, due_date) {
            self.micro_small_compromise_multiplier
        } else {
            1.00
        }
    }

    pub fn is_amended_return_penalty_waived(
        &self,
        class: TaxpayerClass,
        due_date: NaiveDate,
    ) -> bool {
        self.is_micro_or_small_reduction_applicable(class, due_date)
    }

    pub fn calculate_interest(
        &self,
        unpaid_tax: f64,
        class: TaxpayerClass,
        due_date: NaiveDate,
        paid_at: NaiveDate,
    ) -> f64 {
        if unpaid_tax <= 0.0 || paid_at <= due_date {
            return 0.0;
        }

        let mut interest = 0.0;

        if due_date < self.train_interest_effective_date {
            let pre_train_end = paid_at.min(self.train_interest_effective_date);
            let days = (pre_train_end - due_date).num_days().max(0) as f64;
            interest += unpaid_tax * self.pre_train_interest_rate * (days / 365.0);
        }

        if paid_at > self.train_interest_effective_date {
            let post_train_start = due_date.max(self.train_interest_effective_date);
            let days = (paid_at - post_train_start).num_days().max(0) as f64;
            let rate = if self.is_micro_or_small_reduction_applicable(class, due_date) {
                self.get_interest_rate(class)
            } else {
                0.12
            };
            interest += unpaid_tax * rate * (days / 365.0);
        }

        interest
    }
}

impl Default for PenaltyConfig {
    fn default() -> Self {
        Self::default_rules()
    }
}
