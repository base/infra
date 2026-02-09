set positional-arguments := true
set dotenv-load := true

alias t := test
alias f := fix
alias b := build
alias c := clean
alias u := check-udeps
alias wt := watch-test
alias wc := watch-check

# Default service name for docker builds
SERVICE := env_var_or_default("SERVICE", "mempool-rebroadcaster")

# Default to display help menu
default:
    @just --list

# Runs all ci checks
ci: fix check lychee zepter

# Performs lychee checks, installing the lychee command if necessary
lychee:
    @command -v lychee >/dev/null 2>&1 || cargo install lychee
    lychee --config ./lychee.toml .

# Checks formatting, udeps, clippy, and tests
check: check-format check-udeps check-clippy test check-deny

# Runs cargo deny to check dependencies
check-deny:
    @command -v cargo-deny >/dev/null 2>&1 || cargo install cargo-deny
    cargo deny check bans --hide-inclusion-graph

# Fixes formatting and clippy issues
fix: format-fix clippy-fix zepter-fix

# Runs zepter feature checks, installing zepter if necessary
zepter:
    @command -v zepter >/dev/null 2>&1 || cargo install zepter
    zepter --version
    zepter format features
    zepter

# Fixes zepter feature formatting
zepter-fix:
    @command -v zepter >/dev/null 2>&1 || cargo install zepter
    zepter format features --fix

# Runs tests across workspace with all features enabled
test:
    @command -v cargo-nextest >/dev/null 2>&1 || cargo install cargo-nextest
    RUSTFLAGS="-D warnings" cargo nextest run --workspace --all-features

# Checks formatting
check-format:
    cargo +nightly fmt --all -- --check

# Fixes any formatting issues
format-fix:
    cargo fix --allow-dirty --allow-staged --workspace
    cargo +nightly fmt --all

# Checks clippy
check-clippy:
    cargo clippy --workspace --all-targets -- -D warnings

# Fixes any clippy issues
clippy-fix:
    cargo clippy --workspace --all-targets --fix --allow-dirty --allow-staged

# Builds the workspace with release
build:
    cargo build --workspace --release

# Builds all targets in debug mode
build-all-targets:
    cargo build --workspace --all-targets

# Cleans the workspace
clean:
    cargo clean

# Checks if there are any unused dependencies
check-udeps:
    @command -v cargo-udeps >/dev/null 2>&1 || cargo install cargo-udeps
    cargo +nightly udeps --workspace --all-features --all-targets

# Watches tests
watch-test:
    cargo watch -x test

# Watches checks
watch-check:
    cargo watch -x "fmt --all -- --check" -x "clippy --all-targets -- -D warnings" -x test

# Build Docker image for a service
docker-image:
    docker build --platform linux/amd64 --build-arg SERVICE_NAME={{SERVICE}} -t {{SERVICE}} .

# Run mempool rebroadcaster service
run-mempool-rebroadcaster:
    cargo run -p mempool-rebroadcaster -- --geth-mempool-endpoint {{env_var("GETH_MEMPOOL_ENDPOINT")}} --reth-mempool-endpoint {{env_var("RETH_MEMPOOL_ENDPOINT")}}

# Run basectl with specified config (mainnet, sepolia, devnet, or path)
basectl config="mainnet":
    cargo run -p basectl --release -- -c {{config}}
