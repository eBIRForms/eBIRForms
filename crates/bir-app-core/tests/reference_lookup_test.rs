//! A mini-app looks up ATCs and RDOs through `BirApp`, shaped as the New
//! taxpayer RDO list is: by code, and by searching code or name.

use bir_app_core::BirApp;

#[test]
fn atcs_are_found_by_code_and_by_search() {
    let atc = BirApp::atc("wv010").expect("WV010");
    assert_eq!(atc.value, "WV010");
    assert_eq!(atc.title, "WV010 · VAT WITHHOLDING ON PURCHASE OF GOODS");
    assert_eq!(atc.detail, "5.0");
    assert!(BirApp::atc("ZZ999").is_none());

    let all = BirApp::atcs();
    assert!(all.iter().any(|a| a.value == "WV010"));
    let mut codes: Vec<&str> = all.iter().map(|a| a.value.as_str()).collect();
    codes.dedup();
    assert_eq!(codes.len(), all.len(), "one entry per code");

    let found = BirApp::search_atcs("purchase of goods");
    assert!(found.iter().any(|a| a.value == "WV010"));
    assert!(found.len() < all.len());
    assert!(BirApp::search_atcs("Wv01").iter().any(|a| a.value == "WV010"));
    assert_eq!(BirApp::search_atcs("").len(), all.len());
}

#[test]
fn rdos_are_found_by_code_and_by_search() {
    let rdo = BirApp::rdo("043").expect("RDO 043");
    assert_eq!(rdo.value, "043");
    assert_eq!(BirApp::rdo("43"), Some(rdo.clone()), "the table's codes are three characters");
    assert_eq!(BirApp::rdo("43a").expect("43A").value, "43A");
    assert!(BirApp::rdo("999").is_none());
    assert!(BirApp::rdos().contains(&rdo));

    let found = BirApp::search_rdos("east pasig");
    assert_eq!(found.iter().map(|r| r.value.as_str()).collect::<Vec<_>>(), ["43A"]);
    assert!(BirApp::search_rdos("17").iter().all(|r| r.value.contains("17") || r.title.contains("17")));
    assert_eq!(BirApp::search_rdos(" ").len(), BirApp::rdos().len());
}
