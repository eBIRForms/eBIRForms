//! SQLCipher built with its LibTomCrypt crypto provider.
//!
//! `build.rs` compiles the SQLCipher amalgamation and libtomcrypt (both from
//! `sqlcipher-src`) into a static `libsqlcipher.a` and links it. rusqlite's
//! non-bundled `sqlcipher` feature then resolves its `-lsqlcipher` to that
//! archive. On targets other than Android it builds nothing unless the
//! `any-target` feature is on.
//!
//! The crate has no Rust API: depend on it and name it once
//! (`use sqlcipher_tomcrypt as _;`) so the archive is linked.
#![no_std]
