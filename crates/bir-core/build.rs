//! Build script for bir-core: platform library linking, and the packed
//! `data/` JSON (`src/packed.rs`).

include!("pack.rs");

fn main() {
    pack("data", |path| {
        path.extension().is_some_and(|ext| ext == "json")
    });
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() == "windows" {
        println!("cargo:rustc-link-lib=dylib=user32");
        println!("cargo:rustc-link-lib=dylib=gdi32");
        println!("cargo:rustc-link-lib=dylib=crypt32");
        println!("cargo:rustc-link-lib=dylib=advapi32");
        println!("cargo:rustc-link-lib=dylib=ws2_32");
        println!("cargo:rustc-link-lib=dylib=userenv");
        println!("cargo:rustc-link-lib=dylib=bcrypt");
    }
}
