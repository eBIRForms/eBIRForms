# Buwiz

An offline filer for Philippine BIR tax returns. Fill a form, check it,
see deadlines and penalties, print it, and queue it for submission. Every
step works with the internet off except sending.

The app is written in Rust. Its UI uses [Native Shell (nsh)](https://github.com/hexuria/nsh),
and live code patching while you develop uses ra-jit from
[blaze](https://github.com/hexuria/blaze). Both are git submodules.

> Status: early. The window runs; the form screens are being ported.
> macOS first; the Windows and Linux builds are new.

## Requirements

- macOS on Apple Silicon, Windows 10/11 (x64, MSVC) or Linux (GTK 4.14+,
  e.g. Ubuntu 24.04) for the app.
  - Windows: Visual Studio Build Tools, Perl (for the bundled OpenSSL),
    LLVM (set `LIBCLANG_PATH`, e.g. `C:\Program Files\LLVM\bin`) and the
    WebView2 Runtime (built into Windows 11).
  - Linux: `sudo apt install clang libclang-dev pkg-config libgtk-4-dev
    libwebkitgtk-6.0-dev libdbus-1-dev`.
- [rustup](https://rustup.rs). The pinned toolchain (`rust-toolchain.toml`,
  Rust 1.99.0) installs itself on the first build.
- [just](https://github.com/casey/just) (optional, for the shortcuts below).
- For live editing only: `cargo-watch` and ra-jit (see [Live editing](#live-editing)).

## Get the code

```sh
git clone --recurse-submodules <repo-url> buwiz-oss
cd buwiz-oss
```

Already cloned without submodules? Run `git submodule update --init`.

## Run the app

```sh
just run          # or: cd app && cargo run
```

The same command opens the window on Windows and Linux. The first build
compiles nsh and takes a few minutes. Later builds are fast.

## Run the tests

```sh
just test         # or: cargo test
```

This tests the domain crates (forms, validation, deadlines, penalties,
print, submission client). The app builds separately: `just build`.

## Live editing

Live editing patches the running app each time you save a `.rs` file under
`app/src/`, without a restart and without losing what is on screen.

One-time setup:

```sh
cargo install cargo-watch
rustup toolchain install nightly-2026-10-25 --component rust-src
cargo +nightly-2026-10-25 install --locked --path blaze/crates/ra-jit --force
```

Then:

```sh
just dev          # or: cd app && ./dev.sh
```

The sidebar's footer shows `● live` once patches apply. Rules for code
that gets patched:

- Keep app state in `live::store()` or `live::keep("name", init)`, never
  in a `static` or `thread_local!`.
- Changing a type's layout (a new field on `Store`, say) needs a restart.
- Colors in `const`s use plain `Hsla` literals.

## Layout

| Path | What it is |
|---|---|
| `app/` | The desktop app (`buwiz`): screens, store, theme |
| `crates/bir-core` | Forms, field validation, deadlines, penalties, local storage |
| `crates/bir-rules` | Runtime for the pinned rule sets |
| `crates/bir-print` | Filled form sheets (HTML) and PDF output |
| `crates/bir-submit` | Submission queue client |
| `crates/bir-app-core` | The typed API the app calls |
| `nsh/`, `blaze/` | Submodules: UI engine and live patching |

## License

MIT. See [LICENSE](LICENSE).
