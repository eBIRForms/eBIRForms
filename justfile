# Shortcuts. `just` lists them.

default:
    @just --list

# Open the app (plain build, no live patching)
run:
    cd app && RUSTC_WRAPPER= cargo run

# Open the app with live patching (needs ra-jit and cargo-watch)
dev:
    cd app && ./dev.sh

# Open the app in the iOS simulator (default iPhone 17 Pro; `just ios "iPhone 17e"`)
ios device="iPhone 17 Pro":
    cd app && ./ios.sh "{{device}}"

# The iOS simulator with live patching (needs ra-jit and cargo-watch)
ios-dev device="iPhone 17 Pro":
    cd app && ./ios-dev.sh "{{device}}"

# Build the APK and run it on the Android emulator (starts one if none runs)
android:
    cd app && ./android.sh

# Build the app
build:
    cd app && RUSTC_WRAPPER= cargo build

# Test the domain crates
test:
    RUSTC_WRAPPER= cargo test

# Lints for the domain crates and the app
check:
    RUSTC_WRAPPER= cargo clippy --all-targets -- -D warnings
    cd app && RUSTC_WRAPPER= cargo clippy -- -D warnings

# Fail if BIR secrets, personal data or local secret files are in the tree
secrets:
    ./scripts/check-secrets.sh

# Pull the submodules to the commits this repo records
submodules:
    git submodule update --init

# The finish line: one PASS/FAIL per criterion (`just verify V1 V3` for some)
verify *criteria:
    ./scripts/verify.sh {{criteria}}
