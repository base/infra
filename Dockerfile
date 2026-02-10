FROM rust:1.88.0 AS base

ARG FEATURES
ARG RELEASE=true
ARG SERVICE_NAME="mempool-rebroadcaster"

RUN apt-get update \
    && apt-get install -y clang libclang-dev git libssl-dev openssl pkg-config

WORKDIR /app

COPY . .

RUN PROFILE_FLAG=$([ "$RELEASE" = "true" ] && echo "--release" || echo "") && \
    TARGET_DIR=$([ "$RELEASE" = "true" ] && echo "release" || echo "debug") && \
    cargo build $PROFILE_FLAG --features="$FEATURES" --bin=${SERVICE_NAME}; \
    cp target/$TARGET_DIR/${SERVICE_NAME} /tmp/final_binary

#
# Runtime container
#
FROM ubuntu:24.04
RUN apt-get update && apt-get install -y software-properties-common

ARG SERVICE_NAME
# Copy binary with its proper service name
COPY --from=base /tmp/final_binary /usr/local/bin/${SERVICE_NAME}
# Also copy as a fixed entrypoint name
COPY --from=base /tmp/final_binary /usr/local/bin/entrypoint

ENTRYPOINT ["/usr/local/bin/entrypoint"]