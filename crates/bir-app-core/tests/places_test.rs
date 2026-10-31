use std::collections::HashSet;

use bir_app_core::{BirApp, Choice, registered_address, zip_code};

fn unique(list: &[Choice]) -> bool {
    list.iter().map(|c| c.value.as_str()).collect::<HashSet<_>>().len() == list.len()
}

fn titled<'a>(list: &'a [Choice], title: &str) -> Vec<&'a Choice> {
    list.iter().filter(|c| c.title == title).collect()
}

#[test]
fn every_city_is_listed_once_with_its_province() {
    let cities = BirApp::cities();
    assert_eq!(cities.len(), 1617);
    assert!(unique(&cities), "the value tells repeated names apart");
    let pasig = titled(&cities, "City of Pasig");
    assert_eq!(pasig.len(), 1);
    assert_eq!((pasig[0].value.as_str(), pasig[0].detail.as_str()), ("City of Pasig, Metro Manila", "Metro Manila"));
    assert!(cities.iter().all(|c| !c.detail.starts_with("Ncr")), "NCR's districts read Metro Manila");
    assert_eq!(titled(&cities, "San Isidro").len(), 9, "a repeated name stays one row per province");
}

#[test]
fn place_names_read_as_an_address_writes_them() {
    let cities = BirApp::cities();
    let has = |title: &str, detail: &str| cities.iter().any(|c| c.title == title && c.detail == detail);
    assert!(has("Laoag City", "Ilocos Norte"), "a capital loses its \"(Capital)\"");
    assert!(has("Dasmariñas", "Cavite"), "the tables' \"?\" is an Ñ");
    assert!(has("Science City of Muñoz", "Nueva Ecija"));
    assert!(has("Lapu-Lapu City (Opon)", "Cebu"), "a repeated hyphen part is a name");
    assert!(has("Hinoba-an (Asia)", "Negros Occidental"), "a syllable after a hyphen stays lower case");
    assert!(has("Brooke's Point", "Palawan"));
    assert!(has("Tondo I / II", "Metro Manila"), "numerals stay");
    assert!(cities.iter().any(|c| c.title == "Gregorio del Pilar (Concepcion)"));
    assert!(cities.iter().any(|c| c.detail == "Agusan del Norte"));
    assert!(cities.iter().all(|c| c.title.chars().any(char::is_lowercase)), "no name stays upper case");
}

#[test]
fn every_zip_code_is_listed_by_its_entry() {
    let zips = BirApp::zip_codes();
    assert_eq!(zips.len(), bir_app_core::bir_core::reference::get_all_zipcodes().len());
    assert!(unique(&zips), "a code serving several areas is one row per area");
    let diliman = zips.iter().find(|c| c.title == "1101 Diliman").expect("1101 Diliman");
    assert_eq!(diliman.value, "1101 - Diliman");
    assert_eq!(zip_code(&diliman.value), "1101");
    assert!(zips.iter().any(|c| c.title == "1000 Ermita, Manila"), "a district comes before its city");
    assert!(zips.iter().any(|c| c.title == "1600 Pasig"), "the post office's CPO goes");
    assert!(zips.iter().all(|c| c.detail.is_empty()));
}

#[test]
fn every_rdo_is_listed_by_its_code() {
    let rdos = BirApp::rdos();
    assert_eq!(rdos.len(), bir_app_core::bir_core::reference::get_all_rdos().len());
    assert!(unique(&rdos));
    let east = rdos.iter().find(|c| c.value == "43A").expect("RDO 43A");
    assert_eq!(east.title, "43A · East Pasig");
    let unnamed = rdos.iter().find(|c| c.value == "043").expect("RDO 043");
    assert_eq!(unnamed.title, "043", "a placeholder name reads as the code");
    assert_eq!(rdos.iter().filter(|c| c.title == c.value).count(), 16);
}

#[test]
fn the_address_is_the_street_city_and_province() {
    let pasig = BirApp::cities().into_iter().find(|c| c.title == "City of Pasig").expect("Pasig");
    assert_eq!(registered_address(" 1 Sample Street ", &pasig), "1 Sample Street, City of Pasig, Metro Manila");
}
