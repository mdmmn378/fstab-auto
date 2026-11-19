# syntax=docker/dockerfile:1
# Builds fstab-auto and installs it in an Ubuntu image shaped like the target
# machines (Ubuntu 24.04, an unprivileged user with sudo), for end-to-end tests.
FROM rust:1-slim-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked && cp target/release/fstab-auto /fstab-auto

FROM ubuntu:24.04
RUN apt-get update \
    && apt-get install -y --no-install-recommends cifs-utils sudo tmux util-linux \
    && rm -rf /var/lib/apt/lists/*
RUN useradd --create-home --uid 1001 alice \
    && echo 'alice ALL=(ALL) NOPASSWD:ALL' > /etc/sudoers.d/alice
COPY --from=build /fstab-auto /usr/local/bin/fstab-auto
COPY tests/e2e.sh /e2e.sh
CMD ["bash", "/e2e.sh"]
