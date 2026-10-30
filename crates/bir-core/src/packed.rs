//! Embedded data, deflated at build time and inflated on first use.
//!
//! The build scripts of bir-core, bir-print and bir-app-core deflate their
//! big JSON and HTML into `OUT_DIR/packed/` (`pack.rs`): stored raw they were
//! most of the app's binary. [`packed_str!`](crate::packed_str) gives a
//! file's text, inflated once; [`packed_bytes!`](crate::packed_bytes) the
//! deflated bytes, for data parsed once into its own cache, which then
//! [`inflate`]s and drops the text.
//!
//! Deflate because flate2 is already linked (zip's deflate): no new
//! dependency, and the decoder is small and fast.

use std::io::Read;
use std::sync::OnceLock;

/// The text of a packed file.
pub fn inflate(packed: &[u8]) -> String {
    let mut text = String::with_capacity(packed.len() * 8);
    flate2::read::DeflateDecoder::new(packed)
        .read_to_string(&mut text)
        .expect("a packed file inflates to UTF-8");
    text
}

/// `cell`'s text, inflated from `packed` on the first call.
pub fn lazy(cell: &'static OnceLock<String>, packed: &[u8]) -> &'static str {
    cell.get_or_init(|| inflate(packed))
}

/// The text of `path` (from the crate root) that this crate's build script
/// packed, inflated on first use and kept: a `&'static str` like
/// `include_str!`'s.
#[macro_export]
macro_rules! packed_str {
    ($path:expr) => {{
        static TEXT: ::std::sync::OnceLock<::std::string::String> = ::std::sync::OnceLock::new();
        $crate::packed::lazy(&TEXT, $crate::packed_bytes!($path))
    }};
}

/// The deflated bytes of `path` (from the crate root) that this crate's
/// build script packed.
#[macro_export]
macro_rules! packed_bytes {
    ($path:expr) => {
        include_bytes!(concat!(env!("OUT_DIR"), "/packed/", $path))
    };
}

/// For each crate's round-trip test: every file its build script packed
/// (`OUT_DIR/packed_files.rs`) inflates to the bytes of its source.
#[doc(hidden)]
pub fn check_round_trip(manifest: &str, files: &[(&str, &[u8])]) {
    assert!(!files.is_empty(), "nothing packed");
    for (path, packed) in files {
        let source = std::fs::read(std::path::Path::new(manifest).join(path))
            .unwrap_or_else(|e| panic!("{path}: {e}"));
        assert!(
            inflate(packed).as_bytes() == source,
            "{path} does not round-trip"
        );
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_packed_file_inflates_to_its_source() {
        crate::packed::check_round_trip(
            env!("CARGO_MANIFEST_DIR"),
            include!(concat!(env!("OUT_DIR"), "/packed_files.rs")),
        );
    }
}
