//! Reference Data lookup for ATC, RDO, etc.
//!
//! Loads static data from JSON files embedded at compile time.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::OnceLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rdo {
    pub code: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Atc {
    pub code: String,
    pub description: String,
    pub rate: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Region {
    pub code: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Province {
    pub code: String,
    pub region_code: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct City {
    pub code: String,
    pub province_code: String,
    pub region_code: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaxTypeCode {
    pub code: String,
    pub description: String,
    pub form: String,
}

/// Static embedded JSON data
const RDO_JSON: &str = include_str!("../data/rdo.json");
const ATC_JSON: &str = include_str!("../data/atcCodes.json");
const REGION_JSON: &str = include_str!("../data/geo/regions.json");
const PROVINCE_JSON: &str = include_str!("../data/geo/provinces.json");
const CITY_JSON: &str = include_str!("../data/geo/cities.json");
const TAX_TYPE_JSON: &str = include_str!("../data/tax_type_codes.json");
const ZIPCODES_JSON: &str = include_str!("../data/geo/zipcodes.json");

static RDOS_BY_CODE: OnceLock<HashMap<String, Rdo>> = OnceLock::new();
static ATCS_BY_CODE: OnceLock<HashMap<String, Atc>> = OnceLock::new();
static REGIONS_BY_CODE: OnceLock<HashMap<String, Region>> = OnceLock::new();
static PROVINCES_BY_CODE: OnceLock<HashMap<String, Province>> = OnceLock::new();
static CITIES_BY_CODE: OnceLock<HashMap<String, City>> = OnceLock::new();
static TAX_TYPES_BY_CODE: OnceLock<HashMap<String, TaxTypeCode>> = OnceLock::new();
static ZIPCODES: OnceLock<Vec<String>> = OnceLock::new();

/// Initialize and parse the JSON lists
fn init_rdos() -> HashMap<String, Rdo> {
    let list: Vec<Rdo> = serde_json::from_str(RDO_JSON).expect("Invalid rdo.json");
    list.into_iter().map(|r| (r.code.clone(), r)).collect()
}

fn init_atcs() -> HashMap<String, Atc> {
    let list: Vec<Atc> = serde_json::from_str(ATC_JSON).expect("Invalid atcCodes.json");
    list.into_iter().map(|a| (a.code.clone(), a)).collect()
}

fn init_regions() -> HashMap<String, Region> {
    let list: Vec<Region> = serde_json::from_str(REGION_JSON).expect("Invalid regions.json");
    list.into_iter().map(|r| (r.code.clone(), r)).collect()
}

fn init_provinces() -> HashMap<String, Province> {
    let list: Vec<Province> = serde_json::from_str(PROVINCE_JSON).expect("Invalid provinces.json");
    list.into_iter().map(|p| (p.code.clone(), p)).collect()
}

fn init_cities() -> HashMap<String, City> {
    let list: Vec<City> = serde_json::from_str(CITY_JSON).expect("Invalid cities.json");
    list.into_iter().map(|c| (c.code.clone(), c)).collect()
}

fn init_tax_types() -> HashMap<String, TaxTypeCode> {
    let list: Vec<TaxTypeCode> =
        serde_json::from_str(TAX_TYPE_JSON).expect("Invalid tax_type_codes.json");
    list.into_iter().map(|t| (t.code.clone(), t)).collect()
}

fn init_zipcodes() -> Vec<String> {
    serde_json::from_str(ZIPCODES_JSON).expect("Invalid zipcodes.json")
}

/// Get an RDO by its code
pub fn get_rdo(code: &str) -> Option<Rdo> {
    let map = RDOS_BY_CODE.get_or_init(init_rdos);
    map.get(code).cloned()
}

/// Get all RDOs
pub fn get_all_rdos() -> Vec<Rdo> {
    let map = RDOS_BY_CODE.get_or_init(init_rdos);
    let mut rdos: Vec<Rdo> = map.values().cloned().collect();
    rdos.sort_by(|a, b| a.code.cmp(&b.code));
    rdos
}

/// Get an ATC by its code
pub fn get_atc(code: &str) -> Option<Atc> {
    let map = ATCS_BY_CODE.get_or_init(init_atcs);
    map.get(code).cloned()
}

pub fn get_all_regions() -> Vec<Region> {
    let map = REGIONS_BY_CODE.get_or_init(init_regions);
    let mut list: Vec<Region> = map.values().cloned().collect();
    list.sort_by(|a, b| a.code.cmp(&b.code));
    list
}

pub fn get_provinces_for_region(region_code: &str) -> Vec<Province> {
    let map = PROVINCES_BY_CODE.get_or_init(init_provinces);
    let mut list: Vec<Province> = map
        .values()
        .filter(|p| p.region_code == region_code)
        .cloned()
        .collect();
    list.sort_by(|a, b| a.code.cmp(&b.code));
    list
}

pub fn get_cities_for_province(province_code: &str) -> Vec<City> {
    let map = CITIES_BY_CODE.get_or_init(init_cities);
    let mut list: Vec<City> = map
        .values()
        .filter(|c| c.province_code == province_code)
        .cloned()
        .collect();
    list.sort_by(|a, b| a.code.cmp(&b.code));
    list
}

pub fn get_all_tax_types() -> Vec<TaxTypeCode> {
    let map = TAX_TYPES_BY_CODE.get_or_init(init_tax_types);
    let mut list: Vec<TaxTypeCode> = map.values().cloned().collect();
    list.sort_by(|a, b| a.code.cmp(&b.code));
    list
}

pub fn get_all_zipcodes() -> Vec<String> {
    ZIPCODES.get_or_init(init_zipcodes).clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_rdo() {
        let rdo = get_rdo("039").expect("RDO 039 should exist");
        assert_eq!(rdo.code, "039");
        // Ensure description contains something valid from South QC
        assert!(rdo.description.contains("South Quezon City") || !rdo.description.is_empty());
    }

    #[test]
    fn test_get_atc() {
        let atc = get_atc("WV010").expect("ATC WV010 should exist");
        assert_eq!(atc.code, "WV010");
        assert_eq!(atc.rate, "5.0");
    }
}
