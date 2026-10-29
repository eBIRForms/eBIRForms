# Shortcuts. `just` lists them.

default:
    @just --list

# Open the app (plain build, no live patching)
run:
    cd app && RUSTC_WRAPPER= cargo run

# Open the app with live patching (needs ra-jit and cargo-watch)
dev:
    cd app && ./dev.sh

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
