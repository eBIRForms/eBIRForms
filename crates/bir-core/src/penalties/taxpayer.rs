use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TaxpayerClass {
    Micro,
    Small,
    Medium,
    Large,
    #[default]
    Regular,
}
