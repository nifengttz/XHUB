FROM rust:1.97-bookworm AS builder

WORKDIR /build
COPY protocol-v3_6 ./protocol-v3_6
COPY puzzles-v3_6 ./puzzles-v3_6
COPY watchtower-v3_6 ./watchtower-v3_6
COPY hub-v3_6 ./hub-v3_6
COPY wallet-v3_6 ./wallet-v3_6
RUN cargo build --locked --release \
    --manifest-path wallet-v3_6/Cargo.toml \
    --bin wallet-v3-6

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 65532 xhub \
    && useradd --uid 65532 --gid 65532 --no-create-home --shell /usr/sbin/nologin xhub
COPY --from=builder /build/wallet-v3_6/target/release/wallet-v3-6 /usr/local/bin/wallet-v3-6

USER 65532:65532
WORKDIR /var/lib/xhub-wallet
ENTRYPOINT ["/usr/local/bin/wallet-v3-6"]
