# Import environment variables from .env file if it exists
set dotenv-load := true

# Default service name
SERVICE := env_var_or_default("SERVICE", "mempool-rebroadcaster")

# Runs all ci checks
ci: fix test

# Fixes formatting and clippy issues
fix: format-fix clippy-fix

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

# Run tests
test:
    cargo test --all-targets --all-features

# Run mempool rebroadcaster service
run-mempool-rebroadcaster:
    cargo run -p mempool-rebroadcaster -- --geth-mempool-endpoint {{env_var("GETH_MEMPOOL_ENDPOINT")}} --reth-mempool-endpoint {{env_var("RETH_MEMPOOL_ENDPOINT")}}

# Build Docker image
docker-image:
    docker build --platform linux/amd64 --build-arg SERVICE_NAME={{SERVICE}} -t {{SERVICE}} .
