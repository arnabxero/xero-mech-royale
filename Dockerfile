# xero-mech: mechfight/1 bot (Rust). Build: docker build --platform linux/amd64 -t xero-mech .
# The match runs it with no network, a read-only root (writable /tmp), 1 CPU, 512 MB, map at /arena.
FROM rust:1.91-slim-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked --bin xero-mech \
 && strip target/release/xero-mech

FROM debian:bookworm-slim
WORKDIR /bot
COPY --from=build /src/target/release/xero-mech /bot/xero-mech
# fallback copy of the map (the match mounts the same files at /arena)
COPY arena /bot/arena
# variant v5-combo (behaviour switches only; same binary as the base)
ENV XERO_TUNE=solo_duel=1,loot_radius=150,strafe_min=0.4,strafe_max=0.9
ENTRYPOINT ["/bot/xero-mech"]
