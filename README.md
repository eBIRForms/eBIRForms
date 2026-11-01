# eBIRForms

An offline filer for Philippine BIR tax returns, by
[Buwiz](https://ebirforms.buwiz.com). Fill a form, check it, see deadlines
and penalties, print it, and queue it for submission. Every step works with
the internet off except sending.

The app is written in Rust. Its UI uses [Native Shell (nsh)](https://github.com/hexuria/nsh),
and live code patching while you develop uses ra-jit from
[blaze](https://github.com/hexuria/blaze). Both are git submodules.

> Status: early. Every screen runs on macOS, Windows, Linux, iOS and
> Android. Installers come from the [release workflow](#releases), not
> GitHub Releases.

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
git clone --recurse-submodules https://github.com/eBIRForms/eBIRForms.git
cd eBIRForms
```

Already cloned without submodules? Run `git submodule update --init`.

## Run the app

```sh
just run          # or: cd app && cargo run
```

The same command opens the window on Windows and Linux. The first build
compiles nsh and takes a few minutes. Later builds are fast.

On a phone:

```sh
just ios          # the iOS simulator (needs Xcode)
just android      # an Android emulator or device (needs the Android SDK and NDK)
```

## Run the tests

```sh
just test         # or: cargo test
```

This tests the domain crates (forms, validation, deadlines, penalties,
print, submission client). The app builds separately: `just build`.

Before a commit:

```sh
just check        # clippy on the crates and the app, warnings are errors
just secrets      # fails if a BIR secret or personal data is in the tree
just verify       # the finish line: one PASS/FAIL per criterion
```

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
| `crates/sqlcipher-tomcrypt` | SQLCipher for Android, without OpenSSL |
| `crates/bir-app-core` | The typed API the app calls |
| `crates/ebirdesk-mcp` | The app without a window: the eBIRForms Desktop mini app the Buwiz Bot spawns ([README](crates/ebirdesk-mcp/README.md)) |
| `app/packaging` | Installers: DMG, Inno Setup and MSI, `.deb`/`.rpm`/tarball, APK |
| `scripts/` | `check-secrets.sh`, `verify.sh`, `release.sh`, `dmg.sh` |
| `.github/workflows` | CI per platform, `release*.yml` and `promote.yml` |
| `nsh/`, `blaze/` | Submodules: UI engine and live patching |

## Releases

A `vX.Y.Z` tag on `main` runs `release.yml`, which builds the macOS,
Windows, Linux and Android installers and uploads them to Cloudflare R2
under `ebirforms/releases/vX.Y.Z/`, served at `downloads.buwiz.com`. The
DMG is signed and notarized. Without their secrets, the Windows installers
ship unsigned (the run warns) and the APK is skipped. A version is never
overwritten and nothing goes to GitHub Releases.

```sh
just release 0.1.1   # tag main, push, watch release.yml
just upload 0.1.1    # point latest.json at it (promote.yml)
```

`just release` with no version takes `app/Cargo.toml`'s, or the next patch.
The website and the Linux installer (`app/packaging/linux/install-linux.sh`)
read `ebirforms/latest.json`.

## More

- [SECURITY.md](SECURITY.md): what stays on the device, key storage, how to report a problem
- [PRIVACY.md](PRIVACY.md): what leaves the device, and when
- [RELAY-PROTOCOL.md](RELAY-PROTOCOL.md): how sending works
- [AGENTS.md](AGENTS.md): rules for contributors and coding agents

## License

MIT. See [LICENSE](LICENSE).
