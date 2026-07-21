# syntax=docker/dockerfile:1
#
# Multi-arch (amd64/arm64) image for the hosted HTTP transport.
#
# The builder runs on the *native* build platform and cross-compiles to the
# requested target arch, so arm64 images are produced without emulating the Rust
# compiler under QEMU (which is very slow). The runtime is a distroless glibc
# image running as non-root (uid 65532); TLS uses rustls (roots from the OS
# trust store shipped in distroless/cc), so no OpenSSL is involved.

# ---- build stage: cross-compile on the builder's native arch ----
FROM --platform=$BUILDPLATFORM rust:1-bookworm AS builder

# The arm64 cross toolchain (C compiler + linker) is needed because `ring`
# (via rustls) compiles C/asm for the target.
RUN apt-get update && apt-get install -y --no-install-recommends \
        gcc-aarch64-linux-gnu g++-aarch64-linux-gnu \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY tests ./tests

# TARGETARCH is provided by buildx (amd64 | arm64). Map it to a Rust triple,
# add the target, point cross-compilation at the aarch64 toolchain, and build.
ARG TARGETARCH
RUN set -eux; \
    case "$TARGETARCH" in \
      amd64) RUST_TARGET=x86_64-unknown-linux-gnu ;; \
      arm64) RUST_TARGET=aarch64-unknown-linux-gnu ;; \
      *) echo "unsupported TARGETARCH: $TARGETARCH" >&2; exit 1 ;; \
    esac; \
    rustup target add "$RUST_TARGET"; \
    export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc; \
    export CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc; \
    export CXX_aarch64_unknown_linux_gnu=aarch64-linux-gnu-g++; \
    cargo build --release --target "$RUST_TARGET" --bin eve-sde-mcp; \
    cp "target/$RUST_TARGET/release/eve-sde-mcp" /eve-sde-mcp

# Pre-create the data dir so it can be copied in with non-root ownership
# (distroless has no shell to mkdir/chown at runtime).
RUN mkdir -p /data

# ---- runtime stage: distroless glibc, non-root ----
FROM gcr.io/distroless/cc-debian12:nonroot AS runtime

COPY --from=builder /eve-sde-mcp /eve-sde-mcp
COPY --from=builder --chown=65532:65532 /data /data

# Hosted defaults: HTTP transport bound to all interfaces, SDE cache under /data.
# The bind is 0.0.0.0 (not the binary's loopback default) — exposing the port is
# an explicit choice made here, in the image.
ENV SDE_DATA_DIR=/data \
    SDE_TRANSPORT=http \
    SDE_BIND=0.0.0.0:8080

VOLUME ["/data"]
EXPOSE 8080

ENTRYPOINT ["/eve-sde-mcp"]
