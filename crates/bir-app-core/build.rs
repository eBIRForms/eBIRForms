//! Packs the form JSON in `assets/forms/` (bir_core::packed): inflated when
//! first read.

include!("../bir-core/pack.rs");

fn main() {
    pack("assets/forms", |path| {
        path.extension().is_some_and(|ext| ext == "json")
    });
}
