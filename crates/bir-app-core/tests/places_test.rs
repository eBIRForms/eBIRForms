use bir_app_core::BirApp;

#[test]
fn a_zip_code_never_guesses_an_rdo() {
    // Names mislead: Urdaneta Village is in Makati, not Urdaneta City;
    // San Juan, Batangas is not San Juan City; Santa Cruz is in five
    // provinces besides Manila's Sta. Cruz.
    let places = BirApp::places();
    let zips: Vec<_> = places.iter().filter(|p| !p.zip.is_empty()).collect();
    assert!(zips.len() > 1900, "every ZIP code is listed");
    assert!(zips.iter().all(|p| p.rdo_code.is_empty()));
}

#[test]
fn every_rdo_is_listed_on_its_own() {
    let places = BirApp::places();
    let rdos: Vec<_> = places.iter().filter(|p| p.zip.is_empty()).collect();
    assert_eq!(rdos.len(), bir_core::reference::get_all_rdos().len());
    assert!(rdos.iter().any(|p| p.rdo_code == "001" && p.rdo_name == "Laoag City"));
}

#[test]
fn a_zip_area_reads_as_an_address() {
    let city = |zip: &str| BirApp::places().into_iter().find(|p| p.zip == zip).map(|p| p.city);
    assert_eq!(city("1000").as_deref(), Some("Ermita, Manila"));
    assert_eq!(city("1600").as_deref(), Some("Pasig"));
    assert_eq!(city("2900").as_deref(), Some("Laoag City"));
}
