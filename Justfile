set positional-arguments
alias t := test
alias f := fix
alias b := build
alias c := clean

# Default to display help menu
default:
    @just --list

# Runs all ci checks
ci: fix check lychee

# Performs lychee link checks
lychee:
  @command -v lychee >/dev/null 2>&1 || cargo install lychee
  lychee --config ./lychee.toml .

# Checks formatting, clippy, and tests
check: check-format check-clippy test

# Fixes formatting and clippy issues
fix: format-fix clippy-fix

# Runs tests across workspace with all features enabled
test:
    @command -v cargo-nextest >/dev/null 2>&1 || cargo install cargo-nextest
    RUSTFLAGS="-D warnings" cargo nextest run --workspace --all-features

# Checks formatting
check-format:
    cargo +nightly fmt --all -- --check

# Fixes formatting issues
format-fix:
    cargo fix --allow-dirty --allow-staged
    cargo +nightly fmt --all

# Checks clippy
check-clippy:
    cargo clippy --all-targets -- -D warnings

# Fixes clippy issues
clippy-fix:
    cargo clippy --all-targets --fix --allow-dirty --allow-staged

# Builds the workspace with release
build:
    cargo build --release

# Builds all targets in debug mode
build-all-targets:
    cargo build --all-targets

# Builds with maximum performance
build-maxperf:
    cargo build --profile maxperf

# Builds the roxy binary
build-roxy:
    cargo build --bin roxy

# Cleans the workspace
clean:
    cargo clean

# Checks for unused dependencies
check-udeps:
  @command -v cargo-udeps >/dev/null 2>&1 || cargo install cargo-udeps
  cargo +nightly udeps --workspace --all-features --all-targets

# Watches tests
watch-test:
    cargo watch -x test

# Watches checks
watch-check:
    cargo watch -x "fmt --all -- --check" -x "clippy --all-targets -- -D warnings" -x test

# Run all benchmarks
bench:
    cargo bench --workspace

# Run specific benchmark
bench-one name:
    cargo bench --bench {{name}}

# Run fuzz testing (requires cargo-fuzz and nightly Rust)
fuzz target="fuzz_codec_decode" duration="60":
    cd fuzz && cargo +nightly fuzz run {{target}} -- -max_total_time={{duration}}

# List fuzz targets
fuzz-list:
    cd fuzz && cargo +nightly fuzz list
