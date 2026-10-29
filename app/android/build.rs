//! Android's shared runtime supports both 4 KB and 16 KB page sizes: the
//! library's LOAD segments and the end of RELRO must sit on 16 KB pages
//! (as nsh-platform-android's own build script asks of its cdylib).

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("android") {
        println!("cargo:rustc-link-arg-cdylib=-Wl,-z,max-page-size=16384");
        println!("cargo:rustc-link-arg-cdylib=-Wl,-z,common-page-size=16384");
    }
}
