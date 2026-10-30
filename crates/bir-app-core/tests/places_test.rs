use bir_app_core::BirApp;

fn rdo_of(zip: &str) -> Vec<String> {
    BirApp::places().into_iter().filter(|p| p.zip == zip).map(|p| p.rdo_code).collect()
}

#[test]
fn a_zip_area_named_like_one_rdo_gets_that_rdo() {
    assert_eq!(rdo_of("2900"), ["001"], "Laoag City is RDO 001's only match");
}

#[test]
fn a_zip_area_named_like_several_rdos_gets_none() {
    // San Jose del Monte (Bulacan) shares "jose" with San Jose, Antique and
    // San Jose, Occidental Mindoro; Makati CPO with Makati's four RDOs.
    assert_eq!(rdo_of("3023"), [""]);
    assert!(rdo_of("1200").iter().all(String::is_empty));
}
