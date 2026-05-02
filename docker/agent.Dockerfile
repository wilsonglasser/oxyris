# Statically-linked Linux build of the Oxyris WSL agent.
#
# Builder compiles against musl so the binary runs on any distro without
# libc dependencies. `docker buildx build --output=type=local` extracts the
# final artifact into the local filesystem.

# syntax=docker/dockerfile:1.7

FROM rust:1.95-bookworm AS builder

# Build deps:
#   - musl-tools  → musl-gcc for the static musl target
#   - pkg-config  → consumed by some -sys crates
#   - cmake       → required to build libgit2 (vendored by git2-rs)
#   - perl + make → needed by libgit2's build scripts
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        musl-tools \
        pkg-config \
        cmake \
        make \
        perl \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

COPY . .

# `rust-toolchain.toml` pins the toolchain inside the project — rustup
# materializes that channel when we enter `/build`, and any targets must be
# added against THIS toolchain (the global one we'd add before COPY would
# be the wrong one).
RUN rustup target add x86_64-unknown-linux-musl

# Force CMake to use musl-gcc when building libgit2, so the resulting .a
# objects link cleanly into our static musl binary.
ENV CC_x86_64_unknown_linux_musl=musl-gcc \
    AR_x86_64_unknown_linux_musl=ar \
    CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc \
    RUSTFLAGS="-C link-arg=-s"
RUN cargo build --release -p oxyris-agent --target x86_64-unknown-linux-musl \
 && cp target/x86_64-unknown-linux-musl/release/oxyris-agent /oxyris-agent \
 && chmod +x /oxyris-agent

FROM scratch AS export
COPY --from=builder /oxyris-agent /oxyris-agent
