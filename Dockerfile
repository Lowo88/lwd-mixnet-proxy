FROM rust:1.96-slim AS build

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      ca-certificates git pkg-config libssl-dev protobuf-compiler \
 && rm -rf /var/lib/apt/lists/*

# Crates in this tree stamp build-time git metadata, which has no checkout to read here. Idempotent
# mode makes them emit placeholders instead of failing the build.
ENV VERGEN_IDEMPOTENT=1

WORKDIR /src
# Materialise the pinned toolchain in its own layer: the image ships a different patch release.
COPY rust-toolchain.toml ./
RUN rustc --version

COPY Cargo.toml Cargo.lock ./
COPY src ./src

# Cache mounts keep the registry and target dir out of the image.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked \
 && mkdir -p /out \
 && cp target/release/lwd-mixnet-client \
       target/release/lwd-mixnet-server \
       target/release/lwd-mixnet-bench /out/

# Must match the build image's distro: rust:1.96-slim is Debian 13 (glibc 2.41), and a bookworm
# runtime rejects the binary with a GLIBC_2.38 error.
FROM debian:trixie-slim
# curl is here for the container health check and nothing else: the health endpoint is HTTP, and this
# image has no other way to speak it.
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates libssl3t64 curl \
 && rm -rf /var/lib/apt/lists/*
COPY --from=build /out/lwd-mixnet-client /out/lwd-mixnet-server /out/lwd-mixnet-bench /usr/local/bin/

# Neither half needs a privilege it could lose: they bind ports above 1024 and write nothing outside
# the state directory. The uid is fixed and documented because an operator bind-mounting a host
# directory has to chown it to the same number.
RUN groupadd --gid 10001 lwd \
 && useradd --uid 10001 --gid 10001 --create-home lwd \
 && install -d -o 10001 -g 10001 -m 700 /state
ENV HOME=/home/lwd
USER 10001:10001
WORKDIR /home/lwd
