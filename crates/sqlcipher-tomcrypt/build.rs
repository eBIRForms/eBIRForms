//! Compiles SQLCipher (amalgamation from `sqlcipher-src`) with the
//! LibTomCrypt provider into `libsqlcipher.a`.
//!
//! The SQLite options are libsqlite3-sys 0.30's bundled build's (rusqlite
//! 0.32's `bundled-sqlcipher`, which macOS, iOS, Windows and Linux use), so
//! the library behaves the same on every platform; only the crypto provider
//! differs. SQLCipher 4's defaults (page size, KDF iterations, HMAC and KDF
//! algorithms) are compiled in, as there.

use std::env;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let android = target_os == "android";
    if !android && env::var_os("CARGO_FEATURE_ANY_TARGET").is_none() {
        return;
    }

    let dir = sqlcipher_src::source_dir();
    let tomcrypt_include = dir.join(sqlcipher_src::LIBTOMCRYPT_INCLUDE_DIR);

    // libtomcrypt: each file its own translation unit, with LTC_SOURCE and
    // the default configuration (tomcrypt_custom.h), as upstream builds it.
    let mut tomcrypt = cc::Build::new();
    tomcrypt
        .include(&tomcrypt_include)
        .define("LTC_SOURCE", None)
        .define("LTC_NO_TEST", None)
        .warnings(false);
    for source in sqlcipher_src::LIBTOMCRYPT_SOURCES {
        tomcrypt.file(dir.join(source));
    }
    let tomcrypt_objects = tomcrypt.compile_intermediates();

    let mut sqlcipher = cc::Build::new();
    sqlcipher
        .file(dir.join(sqlcipher_src::SOURCE_FILE))
        .include(dir)
        .include(&tomcrypt_include)
        // libsqlite3-sys 0.30.1, build_bundled.
        .define("SQLITE_CORE", None)
        .define("SQLITE_DEFAULT_FOREIGN_KEYS", "1")
        .define("SQLITE_ENABLE_API_ARMOR", None)
        .define("SQLITE_ENABLE_COLUMN_METADATA", None)
        .define("SQLITE_ENABLE_DBSTAT_VTAB", None)
        .define("SQLITE_ENABLE_FTS3", None)
        .define("SQLITE_ENABLE_FTS3_PARENTHESIS", None)
        .define("SQLITE_ENABLE_FTS5", None)
        .define("SQLITE_ENABLE_JSON1", None)
        .define("SQLITE_ENABLE_LOAD_EXTENSION", "1")
        .define("SQLITE_ENABLE_MEMORY_MANAGEMENT", None)
        .define("SQLITE_ENABLE_RTREE", None)
        .define("SQLITE_ENABLE_STAT4", None)
        .define("SQLITE_SOUNDEX", None)
        .define("SQLITE_THREADSAFE", "1")
        .define("SQLITE_USE_URI", None)
        .define("HAVE_USLEEP", "1")
        .define("HAVE_ISNAN", None)
        .define("_POSIX_THREAD_SAFE_FUNCTIONS", None)
        .define("HAVE_LOCALTIME_R", None)
        // SQLCipher, LibTomCrypt provider. Temp store in memory only on
        // Android (libsqlite3-sys does the same there), else as bundled.
        .define("SQLITE_HAS_CODEC", None)
        .define("SQLCIPHER_CRYPTO_LIBTOMCRYPT", None)
        .define("SQLITE_TEMP_STORE", if android { "3" } else { "2" })
        // Required since SQLCipher 4.6 (libsqlite3-sys 0.30's 4.5.7 had
        // them built in).
        .define("SQLITE_EXTRA_INIT", "sqlcipher_extra_init")
        .define("SQLITE_EXTRA_SHUTDOWN", "sqlcipher_extra_shutdown")
        .warnings(false);
    let sqlcipher_objects = sqlcipher.compile_intermediates();

    // One archive, named as rusqlite's `sqlcipher` feature links it.
    cc::Build::new()
        .objects(tomcrypt_objects)
        .objects(sqlcipher_objects)
        .compile("sqlcipher");

    // SQLCipher logs to logcat on Android.
    if android {
        println!("cargo:rustc-link-lib=log");
    }
}
